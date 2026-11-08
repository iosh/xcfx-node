//! Immutable local changes over a fixed, read-only remote state.

use xcfx_state::account::Account;

use std::{io, sync::Arc};

use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_statedb::Result as StateResult;
use cfx_statedb::global_params::*;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, BigEndianHash, H256, Space, U256};
use imbl::{HashSet, OrdMap};
use keccak_hash::{KECCAK_EMPTY, keccak};
use primitives::{DepositList, StorageKey, StorageKeyWithSpace, StorageLayout, VoteStakeList};
use thiserror::Error;
use xcfx_state::database::{CodeEntry, Database, StorageEntry, StorageVisitor};

use crate::fork::{ForkClient, ForkReadError, StateValue};

use super::StateReadError;

#[cfg(test)]
mod tests;

#[derive(Debug, Error)]
enum ForkStateError {
  #[error("the remote source cannot enumerate this complete state range")]
  IncompleteRange,
}

/// A local lookup can inherit the fixed base, fix absence, or supply a value.
enum LocalLookup<T> {
  Inherit,
  Absent,
  Value(T),
}

#[derive(Clone, Default)]
struct LocalChanges {
  entries: OrdMap<Vec<u8>, Option<StateValue>>,
  cleared_storage: HashSet<AddressWithSpace>,
  cleared_code: HashSet<AddressWithSpace>,
}

impl LocalChanges {
  fn get(&self, key: StorageKeyWithSpace<'_>) -> LocalLookup<&StateValue> {
    if let Some(value) = self.entries.get(&key.to_key_bytes()) {
      return match value {
        Some(value) => LocalLookup::Value(value),
        None => LocalLookup::Absent,
      };
    }
    if self.hides_remote(key) {
      LocalLookup::Absent
    } else {
      LocalLookup::Inherit
    }
  }

  fn remove_prefix(&mut self, prefix: &[u8]) {
    let keys = self
      .entries
      .range(prefix.to_vec()..)
      .take_while(|(key, _)| key.starts_with(prefix))
      .map(|(key, _)| key.clone())
      .collect::<Vec<_>>();
    for key in keys {
      self.entries.remove(&key);
    }
  }

  fn hides_remote(&self, key: StorageKeyWithSpace<'_>) -> bool {
    match key.key {
      StorageKey::StorageRootKey(bytes)
      | StorageKey::StorageKey {
        address_bytes: bytes,
        ..
      } => self
        .cleared_storage
        .contains(&account_address(bytes, key.space)),
      StorageKey::CodeRootKey(bytes)
      | StorageKey::CodeKey {
        address_bytes: bytes,
        ..
      } => self
        .cleared_code
        .contains(&account_address(bytes, key.space)),
      _ => false,
    }
  }
}

/// A version owns the cumulative local effect, so reads never walk parent chains.
pub(crate) struct ForkStateVersion {
  client: ForkClient,
  changes: LocalChanges,
}

impl ForkStateVersion {
  pub(crate) fn new(client: ForkClient, globals: [U256; TOTAL_GLOBAL_PARAMS]) -> Self {
    let mut changes = LocalChanges::default();
    for (key, index) in [
      (InterestRate::STORAGE_KEY, InterestRate::ID),
      (
        AccumulateInterestRate::STORAGE_KEY,
        AccumulateInterestRate::ID,
      ),
      (TotalIssued::STORAGE_KEY, TotalIssued::ID),
      (TotalStaking::STORAGE_KEY, TotalStaking::ID),
      (TotalStorage::STORAGE_KEY, TotalStorage::ID),
      (TotalEvmToken::STORAGE_KEY, TotalEvmToken::ID),
      (UsedStoragePoints::STORAGE_KEY, UsedStoragePoints::ID),
      (
        ConvertedStoragePoints::STORAGE_KEY,
        ConvertedStoragePoints::ID,
      ),
      (TotalPosStaking::STORAGE_KEY, TotalPosStaking::ID),
      (
        DistributablePoSInterest::STORAGE_KEY,
        DistributablePoSInterest::ID,
      ),
      (LastDistributeBlock::STORAGE_KEY, LastDistributeBlock::ID),
      (PowBaseReward::STORAGE_KEY, PowBaseReward::ID),
      (TotalBurnt1559::STORAGE_KEY, TotalBurnt1559::ID),
      (BaseFeeProp::STORAGE_KEY, BaseFeeProp::ID),
    ] {
      changes.entries.insert(
        key.to_key_bytes(),
        (!globals[index].is_zero())
          .then_some(StateValue::Storage(StorageEntry::new(globals[index], None))),
      );
    }
    Self { client, changes }
  }

  fn local_native_account(&self, address: Address) -> LocalLookup<&Account> {
    let key = StorageKey::new_account_key(&address).with_native_space();
    match self.changes.get(key) {
      LocalLookup::Value(value) => LocalLookup::Value(value.account()),
      LocalLookup::Absent => LocalLookup::Absent,
      LocalLookup::Inherit => LocalLookup::Inherit,
    }
  }

  pub(super) fn native_balance(&self, address: Address) -> Result<U256, StateReadError> {
    match self.local_native_account(address) {
      LocalLookup::Value(account) => Ok(account.balance),
      LocalLookup::Absent => Ok(U256::zero()),
      LocalLookup::Inherit => Ok(self.client.balance(&address.with_native_space())?),
    }
  }

  pub(super) fn native_nonce(&self, address: Address) -> Result<U256, StateReadError> {
    match self.local_native_account(address) {
      LocalLookup::Value(account) => Ok(account.nonce),
      LocalLookup::Absent => Ok(U256::zero()),
      LocalLookup::Inherit => Ok(
        self
          .client
          .core_nonce_at(address, self.client.base().epoch_height)?,
      ),
    }
  }

  pub(super) fn native_code(&self, address: Address) -> Result<Arc<Vec<u8>>, StateReadError> {
    let expected_hash = match self.local_native_account(address) {
      LocalLookup::Absent => return Ok(Arc::default()),
      LocalLookup::Value(account) => {
        let key = StorageKey::new_code_key(&address, &account.code_hash).with_native_space();
        match self.changes.get(key) {
          LocalLookup::Value(value) => return Ok(Arc::clone(value.code().code())),
          LocalLookup::Absent => return Ok(Arc::default()),
          LocalLookup::Inherit => {}
        }
        if account.code_hash == KECCAK_EMPTY {
          return Ok(Arc::default());
        }
        Some(account.code_hash)
      }
      LocalLookup::Inherit => {
        let key = StorageKey::new_code_root_key(&address).with_native_space();
        if self.changes.hides_remote(key) {
          return Ok(Arc::default());
        }
        None
      }
    };

    let code = self
      .client
      .core_code_at(address, self.client.base().epoch_height)?;
    if expected_hash.is_some_and(|expected| keccak(code.as_slice()) != expected) {
      return Err(
        ForkReadError::Inconsistent("Core code hash does not match the local account").into(),
      );
    }
    Ok(code)
  }

  pub(super) fn native_storage(
    &self,
    address: Address,
    position: H256,
  ) -> Result<Option<H256>, StateReadError> {
    let key = StorageKey::new_storage_key(&address, position.as_bytes()).with_native_space();
    match self.changes.get(key) {
      LocalLookup::Value(value) => Ok(Some(H256::from_uint(&value.storage().value()))),
      LocalLookup::Absent => Ok(None),
      LocalLookup::Inherit => Ok(self.client.core_storage_at(
        address,
        position,
        self.client.base().epoch_height,
      )?),
    }
  }
}

/// Copy-on-write changes remain private until the caller publishes a version.
pub(crate) struct ForkStateCandidate {
  client: ForkClient,
  changes: LocalChanges,
}

impl ForkStateCandidate {
  pub(crate) fn new(parent: Arc<ForkStateVersion>) -> Self {
    Self {
      client: parent.client.clone(),
      changes: parent.changes.clone(),
    }
  }

  pub(crate) fn into_version(self) -> ForkStateVersion {
    ForkStateVersion {
      client: self.client,
      changes: self.changes,
    }
  }

  fn write_entry(&mut self, key: StorageKeyWithSpace<'_>, value: Option<StateValue>) {
    self.changes.entries.insert(key.to_key_bytes(), value);
  }

  fn value(&self, key: StorageKeyWithSpace<'_>) -> StateResult<Option<StateValue>> {
    match self.changes.get(key) {
      LocalLookup::Value(value) => Ok(Some(value.clone())),
      LocalLookup::Absent => Ok(None),
      LocalLookup::Inherit => self.client.state_value(key).map_err(state_error),
    }
  }

  fn clear_account(&mut self, address: &AddressWithSpace) {
    for key in [
      StorageKey::new_storage_root_key(&address.address),
      StorageKey::new_code_root_key(&address.address),
    ] {
      self
        .changes
        .remove_prefix(&key.with_space(address.space).to_key_bytes());
    }
    self.changes.cleared_storage.insert(*address);
    self.changes.cleared_code.insert(*address);
    for key in [
      StorageKey::new_account_key(&address.address),
      StorageKey::new_deposit_list_key(&address.address),
      StorageKey::new_vote_list_key(&address.address),
    ] {
      self.write_entry(key.with_space(address.space), None);
    }
  }
}

impl Database for ForkStateCandidate {
  fn is_initialized(&self) -> StateResult<bool> {
    // Construction requires all base globals, including any zero values.
    Ok(true)
  }

  fn get_account(&self, address: &AddressWithSpace) -> StateResult<Option<Account>> {
    let key = StorageKey::new_account_key(&address.address).with_space(address.space);
    Ok(self.value(key)?.map(|value| value.account().clone()))
  }

  fn get_account_balance(&self, address: &AddressWithSpace) -> StateResult<U256> {
    let key = StorageKey::new_account_key(&address.address).with_space(address.space);
    match self.changes.get(key) {
      LocalLookup::Value(value) => Ok(value.account().balance),
      LocalLookup::Absent => Ok(U256::zero()),
      LocalLookup::Inherit => self.client.balance(address).map_err(state_error),
    }
  }

  fn get_code(&self, address: &AddressWithSpace, hash: &H256) -> StateResult<Option<CodeEntry>> {
    let key = StorageKey::new_code_key(&address.address, hash).with_space(address.space);
    Ok(self.value(key)?.map(|value| value.code().clone()))
  }

  fn get_storage_entry(
    &self,
    address: &AddressWithSpace,
    key: &[u8],
  ) -> StateResult<Option<StorageEntry>> {
    let key = StorageKey::new_storage_key(&address.address, key).with_space(address.space);
    Ok(match self.value(key)? {
      // The read boundary reports this approximation. Local writes always
      // retain concrete values and take precedence over the hidden base entry.
      Some(StateValue::HiddenWhitelistEntry) | None => None,
      Some(value) => Some(value.storage()),
    })
  }

  fn storage_points_initialized(&self, address: &Address) -> StateResult<bool> {
    // This resolves a fact of the fixed base. The caller keeps local amounts
    // and stores the resolved flag in its own account/checkpoint/version.
    self
      .client
      .storage_points_initialized(address)
      .map_err(state_error)
  }

  fn get_deposit_list(&self, address: &AddressWithSpace) -> StateResult<Option<DepositList>> {
    address.assert_native();
    let key = StorageKey::new_deposit_list_key(&address.address).with_native_space();
    Ok(self.value(key)?.map(|value| value.deposit_list().clone()))
  }

  fn get_vote_list(&self, address: &AddressWithSpace) -> StateResult<Option<VoteStakeList>> {
    address.assert_native();
    let key = StorageKey::new_vote_list_key(&address.address).with_native_space();
    Ok(self.value(key)?.map(|value| value.vote_list().clone()))
  }

  fn get_global_param(&self, key: StorageKeyWithSpace<'_>) -> StateResult<Option<U256>> {
    Ok(self.value(key)?.map(|value| value.storage().value()))
  }

  fn visit_storage(
    &mut self,
    address: &AddressWithSpace,
    prefix: &[u8],
    visitor: &mut StorageVisitor<'_>,
  ) -> StateResult<()> {
    if !self.changes.cleared_storage.contains(address) {
      return Err(state_error(ForkStateError::IncompleteRange));
    }
    let prefix = if prefix.is_empty() {
      StorageKey::new_storage_root_key(&address.address)
    } else {
      StorageKey::new_storage_key(&address.address, prefix)
    }
    .with_space(address.space)
    .to_key_bytes();
    for (key, value) in self.changes.entries.range(prefix.clone()..) {
      if !key.starts_with(&prefix) {
        break;
      }
      let Some(value) = value else {
        continue;
      };
      let key = StorageKeyWithSpace::from_key_bytes::<primitives::SkipInputCheck>(key);
      if let StorageKey::StorageKey { storage_key, .. } = key.key {
        visitor(storage_key, &|| Ok(value.storage()))?;
      }
    }
    Ok(())
  }

  fn record_contract_creation(&mut self, address: &AddressWithSpace) {
    self.clear_account(address);
  }

  fn set_account(
    &mut self,
    account: &Account,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    let address = account.address();
    let key = StorageKey::new_account_key(&address.address).with_space(address.space);
    let value = (!account.is_default()).then(|| StateValue::Account(Arc::new(account.clone())));
    self.write_entry(key, value);
    Ok(())
  }

  fn set_code(
    &mut self,
    address: &AddressWithSpace,
    hash: &H256,
    code: &CodeEntry,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    let key = StorageKey::new_code_key(&address.address, hash).with_space(address.space);
    self.write_entry(
      key,
      (!code.is_default()).then(|| StateValue::Code(code.clone())),
    );
    Ok(())
  }

  fn set_storage_entry(
    &mut self,
    address: &AddressWithSpace,
    key: &[u8],
    entry: &StorageEntry,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    let key = StorageKey::new_storage_key(&address.address, key).with_space(address.space);
    self.write_entry(
      key,
      (!entry.value().is_zero()).then_some(StateValue::Storage(*entry)),
    );
    Ok(())
  }

  fn set_deposit_list(
    &mut self,
    address: &AddressWithSpace,
    list: &DepositList,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    address.assert_native();
    let key = StorageKey::new_deposit_list_key(&address.address).with_native_space();
    self.write_entry(
      key,
      (!list.0.is_empty()).then(|| StateValue::DepositList(Arc::new(list.clone()))),
    );
    Ok(())
  }

  fn set_vote_list(
    &mut self,
    address: &AddressWithSpace,
    list: &VoteStakeList,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    address.assert_native();
    let key = StorageKey::new_vote_list_key(&address.address).with_native_space();
    self.write_entry(
      key,
      (!list.0.is_empty()).then(|| StateValue::VoteList(Arc::new(list.clone()))),
    );
    Ok(())
  }

  fn set_global_param(
    &mut self,
    key: StorageKeyWithSpace<'_>,
    value: &U256,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    self.write_entry(
      key,
      (!value.is_zero()).then_some(StateValue::Storage(StorageEntry::new(*value, None))),
    );
    Ok(())
  }

  fn set_storage_layout(
    &mut self,
    _address: &AddressWithSpace,
    _layout: StorageLayout,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    Ok(())
  }

  fn delete_account(
    &mut self,
    address: &AddressWithSpace,
    _debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> StateResult<()> {
    self.clear_account(address);
    Ok(())
  }

  fn flush(&mut self, _debug_record: Option<&mut ComputeEpochDebugRecord>) -> StateResult<()> {
    Ok(())
  }
}

fn account_address(bytes: &[u8], space: Space) -> AddressWithSpace {
  assert_eq!(
    bytes.len(),
    20,
    "executor state key must contain a full account address"
  );
  AddressWithSpace {
    address: Address::from_slice(bytes),
    space,
  }
}

fn state_error(error: impl std::error::Error + Send + Sync + 'static) -> cfx_statedb::Error {
  cfx_storage_types::Error::Io(io::Error::other(error)).into()
}
