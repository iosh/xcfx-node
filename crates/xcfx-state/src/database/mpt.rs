//! Converts complete records while preserving Conflux's canonical writeback.
use super::{CodeEntry, Database, StorageEntry, StorageOwner, StorageVisitor};
use crate::account::Account;
use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_parameters::internal_contract_addresses::SYSTEM_STORAGE_ADDRESS;
use cfx_statedb::{access_mode, Result as DbResult, StateDbExt, StateDbGeneric, StateStorage};
use cfx_types::{AddressWithSpace, Space, H256, U256};
use primitives::{
  CodeInfo, DepositList, SkipInputCheck, StorageKey, StorageKeyWithSpace, StorageLayout,
  StorageValue, VoteStakeList,
};

/// A canonical MPT adapter over owned or borrowed storage.
/// It buffers and flushes through the upstream StateDb; the runtime retains
/// responsibility for materializing the backing candidate and publishing it.
pub struct MptDatabase<S> {
  db: StateDbGeneric<S>,
}

impl<S: StateStorage> MptDatabase<S> {
  pub fn new(storage: S) -> Self {
    Self {
      db: StateDbGeneric::with_storage(storage),
    }
  }
}

fn storage_entry(address: &AddressWithSpace, value: StorageValue) -> StorageEntry {
  let owner = value.owner.or_else(|| {
    (address.space == Space::Native
      && address.address != SYSTEM_STORAGE_ADDRESS
      && !value.value.is_zero())
    .then_some(address.address)
  });
  StorageEntry::new(value.value, owner)
}

impl<S: StateStorage> Database for MptDatabase<S> {
  fn is_initialized(&self) -> DbResult<bool> {
    self.db.is_initialized()
  }
  fn get_account(&self, address: &AddressWithSpace) -> DbResult<Option<Account>> {
    Ok(
      self
        .db
        .get_account(address)?
        .map(|account| Account::from_conflux(address, account)),
    )
  }
  fn get_account_balance(&self, address: &AddressWithSpace) -> DbResult<U256> {
    Ok(
      self
        .db
        .get_account(address)?
        .map_or(U256::zero(), |account| account.balance),
    )
  }
  fn get_code(&self, address: &AddressWithSpace, hash: &H256) -> DbResult<Option<CodeEntry>> {
    Ok(
      self
        .db
        .get_code(address, hash)?
        .map(|value| CodeEntry::new(value.code, value.owner)),
    )
  }
  fn get_storage_entry(
    &self,
    address: &AddressWithSpace,
    key: &[u8],
  ) -> DbResult<Option<StorageEntry>> {
    let key = StorageKey::new_storage_key(&address.address, key).with_space(address.space);
    Ok(
      self
        .db
        .get::<StorageValue>(key)?
        .map(|value| storage_entry(address, value)),
    )
  }
  fn get_deposit_list(&self, address: &AddressWithSpace) -> DbResult<Option<DepositList>> {
    self.db.get_deposit_list(address)
  }
  fn get_vote_list(&self, address: &AddressWithSpace) -> DbResult<Option<VoteStakeList>> {
    self.db.get_vote_list(address)
  }
  fn get_global_param(&self, key: StorageKeyWithSpace<'_>) -> DbResult<Option<U256>> {
    self.db.get::<U256>(key)
  }
  fn visit_storage(
    &mut self,
    address: &AddressWithSpace,
    prefix: &[u8],
    visitor: &mut StorageVisitor<'_>,
  ) -> DbResult<()> {
    let key = if prefix.is_empty() {
      StorageKey::new_storage_root_key(&address.address)
    } else {
      StorageKey::new_storage_key(&address.address, prefix)
    }
    .with_space(address.space);
    for (key, raw) in self.db.delete_all::<access_mode::Read>(key, None)? {
      let key = StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&key);
      if let StorageKey::StorageKey { storage_key, .. } = key.key {
        visitor(storage_key, &|| {
          Ok(storage_entry(address, rlp::decode(&raw)?))
        })?;
      }
    }
    Ok(())
  }
  fn record_contract_creation(&mut self, _address: &AddressWithSpace) {
    // The complete local state already supplies absence for a new contract.
  }
  fn set_account(
    &mut self,
    account: &Account,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    let address = account.address();
    self.db.set::<primitives::Account>(
      StorageKey::new_account_key(&address.address).with_space(address.space),
      &account.as_conflux(),
      debug,
    )
  }
  fn set_code(
    &mut self,
    address: &AddressWithSpace,
    hash: &H256,
    code: &CodeEntry,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    let owner = code
      .owner
      .expect("MPT code records have known collateral attribution");
    self.db.set::<CodeInfo>(
      StorageKey::new_code_key(&address.address, hash).with_space(address.space),
      &CodeInfo {
        code: code.code.clone(),
        owner,
      },
      debug,
    )
  }
  fn set_storage_entry(
    &mut self,
    address: &AddressWithSpace,
    key: &[u8],
    entry: &StorageEntry,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    let key = StorageKey::new_storage_key(&address.address, key).with_space(address.space);
    if entry.value.is_zero() {
      return self.db.delete(key, debug);
    }
    let owner = match entry.owner {
      StorageOwner::None => None,
      StorageOwner::Address(owner) => (owner != address.address).then_some(owner),
      StorageOwner::Unknown => panic!("MPT storage records have known collateral attribution"),
    };
    self.db.set::<StorageValue>(
      key,
      &StorageValue {
        value: entry.value,
        owner,
      },
      debug,
    )
  }
  fn set_deposit_list(
    &mut self,
    address: &AddressWithSpace,
    list: &DepositList,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    self.db.set(
      StorageKey::new_deposit_list_key(&address.address).with_space(address.space),
      list,
      debug,
    )
  }
  fn set_vote_list(
    &mut self,
    address: &AddressWithSpace,
    list: &VoteStakeList,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    self.db.set(
      StorageKey::new_vote_list_key(&address.address).with_space(address.space),
      list,
      debug,
    )
  }
  fn set_global_param(
    &mut self,
    key: StorageKeyWithSpace<'_>,
    value: &U256,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    self.db.set(key, value, debug)
  }
  fn set_storage_layout(
    &mut self,
    address: &AddressWithSpace,
    layout: StorageLayout,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    self.db.set_storage_layout(address, layout, debug)
  }
  fn delete_account(
    &mut self,
    address: &AddressWithSpace,
    mut debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    for key in [
      StorageKey::new_storage_root_key(&address.address),
      StorageKey::new_code_root_key(&address.address),
    ] {
      self
        .db
        .delete_all::<access_mode::Write>(key.with_space(address.space), debug.as_deref_mut())?;
    }
    for key in [
      StorageKey::new_account_key(&address.address),
      StorageKey::new_deposit_list_key(&address.address),
      StorageKey::new_vote_list_key(&address.address),
    ] {
      self
        .db
        .delete(key.with_space(address.space), debug.as_deref_mut())?;
    }
    Ok(())
  }
  fn flush(&mut self, debug: Option<&mut ComputeEpochDebugRecord>) -> DbResult<()> {
    self.db.apply_changes_to_storage(debug)
  }
}
