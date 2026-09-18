//! Immutable local changes over a fixed, read-only remote state.

use std::{io, sync::Arc};

use cfx_statedb::global_params::*;
use cfx_storage_types::{AccountClearMode, MptKeyValue, Result as StorageResult, StateStorage};
use cfx_types::{Address, AddressWithSpace, Space, U256};
use imbl::{HashSet, OrdMap};
use primitives::{Account, SkipInputCheck, StorageKey, StorageKeyWithSpace};
use rlp::Rlp;
use thiserror::Error;

use crate::fork::ForkClient;

#[derive(Debug, Error)]
enum ForkStateError {
  #[error("the remote source cannot enumerate this complete state range")]
  IncompleteRange,
}

#[derive(Clone, Default)]
struct LocalChanges {
  entries: OrdMap<Vec<u8>, Option<Arc<[u8]>>>,
  cleared_storage: HashSet<AddressWithSpace>,
  cleared_code: HashSet<AddressWithSpace>,
}

impl LocalChanges {
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
      // StateDb uses InterestRate's presence to recognize initialized state,
      // so zero values must also be retained in the initial version.
      changes.entries.insert(
        key.to_key_bytes(),
        Some(rlp::encode(&globals[index]).as_ref().into()),
      );
    }
    Self { client, changes }
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

  fn write_entry(&mut self, key: StorageKeyWithSpace<'_>, value: Option<Arc<[u8]>>) {
    self.changes.entries.insert(key.to_key_bytes(), value);
  }

  fn clear_account(&mut self, address: &AddressWithSpace, storage: bool) {
    let key = if storage {
      StorageKey::new_storage_root_key(&address.address)
    } else {
      StorageKey::new_code_root_key(&address.address)
    }
    .with_space(address.space);
    let prefix = key.to_key_bytes();
    self.changes.remove_prefix(&prefix);
    if storage {
      self.changes.cleared_storage.insert(*address);
    } else {
      self.changes.cleared_code.insert(*address);
    }
  }

  fn complete_range(
    &self,
    prefix: StorageKeyWithSpace<'_>,
  ) -> Result<Vec<MptKeyValue>, ForkStateError> {
    // Cached remote points never prove that a range is complete. After a full
    // namespace clear, every visible entry in it is necessarily local.
    if !self.changes.hides_remote(prefix) {
      return Err(ForkStateError::IncompleteRange);
    }
    let prefix = prefix.to_key_bytes();
    let mut entries = Vec::new();
    for (key, value) in self.changes.entries.range(prefix.clone()..) {
      if !key.starts_with(&prefix) {
        break;
      }
      if let Some(value) = value {
        entries.push((key.clone(), Box::<[u8]>::from(value.as_ref())));
      }
    }
    Ok(entries)
  }
}

impl StateStorage for ForkStateCandidate {
  fn account_clear_mode(&self) -> AccountClearMode {
    AccountClearMode::Deferred
  }

  fn get_account_balance(&self, address: &AddressWithSpace) -> StorageResult<U256> {
    let key = StorageKey::new_account_key(&address.address)
      .with_space(address.space)
      .to_key_bytes();
    if let Some(value) = self.changes.entries.get(&key) {
      return match value {
        None => Ok(U256::zero()),
        Some(raw) => Ok(Account::new_from_rlp(address.address, &Rlp::new(raw.as_ref()))?.balance),
      };
    }
    self.client.balance(address).map_err(storage_error)
  }

  fn get(&self, key: StorageKeyWithSpace<'_>) -> StorageResult<Option<Box<[u8]>>> {
    if let Some(value) = self.changes.entries.get(&key.to_key_bytes()) {
      return Ok(
        value
          .as_ref()
          .map(|value| Box::<[u8]>::from(value.as_ref())),
      );
    }
    if self.changes.hides_remote(key) {
      return Ok(None);
    }
    self.client.state_value(key).map_err(storage_error)
  }

  fn set(&mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>) -> StorageResult<()> {
    self.write_entry(key, Some(value.into()));
    Ok(())
  }

  fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> StorageResult<()> {
    self.write_entry(key, None);
    Ok(())
  }

  fn delete_test_only(&mut self, key: StorageKeyWithSpace<'_>) -> StorageResult<Option<Box<[u8]>>> {
    let key = key.to_key_bytes();
    let Some(value) = self.changes.entries.remove(&key) else {
      return Ok(None);
    };
    Ok(Some(value.map_or_else(Box::<[u8]>::default, |value| {
      Box::<[u8]>::from(value.as_ref())
    })))
  }

  fn clear_account_storage(&mut self, address: &AddressWithSpace) -> StorageResult<()> {
    self.clear_account(address, true);
    Ok(())
  }

  fn clear_account_code(&mut self, address: &AddressWithSpace) -> StorageResult<()> {
    self.clear_account(address, false);
    Ok(())
  }

  fn read_all(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
  ) -> StorageResult<Option<Vec<MptKeyValue>>> {
    let values = self.complete_range(prefix).map_err(storage_error)?;
    Ok((!values.is_empty()).then_some(values))
  }

  fn delete_all(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
  ) -> StorageResult<Option<Vec<MptKeyValue>>> {
    let deleted = self.complete_range(prefix).map_err(storage_error)?;
    let prefix = prefix.to_key_bytes();
    self.changes.remove_prefix(&prefix);
    Ok((!deleted.is_empty()).then_some(deleted))
  }

  fn read_all_with_callback(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
    callback: &mut dyn FnMut(MptKeyValue),
    only_account_key: bool,
  ) -> StorageResult<()> {
    let entries = self.complete_range(prefix).map_err(storage_error)?;
    for (key, value) in entries {
      if only_account_key
        && !matches!(
          StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&key).key,
          StorageKey::AccountKey(_)
        )
      {
        continue;
      }
      callback((key, value));
    }
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

fn storage_error(
  error: impl std::error::Error + Send + Sync + 'static,
) -> cfx_storage_types::Error {
  cfx_storage_types::Error::Io(io::Error::other(error))
}
