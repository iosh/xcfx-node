use super::Substate;
use crate::database::Database;
use crate::database::StorageEntry;
use crate::database::StorageOwner;
use crate::database::WriteCacheItem;
use cfx_types::address_util::AddressUtil;

use cfx_parameters::{
  internal_contract_addresses::SYSTEM_STORAGE_ADDRESS, staking::COLLATERAL_UNITS_PER_STORAGE_KEY,
};
use cfx_statedb::Result as DbResult;
use cfx_types::{Address, Space, U256};

use std::collections::{hash_map::Entry::*, HashSet};

use super::OverlayAccount;

impl OverlayAccount {
  pub fn set_storage(
    &mut self,
    key: Vec<u8>,
    value: U256,
    old_value: StorageEntry,
    payer: Address,
    substate: &mut Substate,
  ) {
    let owner = if self.should_have_owner(&key) && !value.is_zero() {
      if self.original_storage_entry_at(&key).owner == StorageOwner::Unknown {
        StorageOwner::Unknown
      } else {
        StorageOwner::Address(payer)
      }
    } else {
      StorageOwner::None
    };
    if let Some(address) = old_value.refundable_owner() {
      substate.record_storage_release(&address, COLLATERAL_UNITS_PER_STORAGE_KEY);
    }
    if let StorageOwner::Address(address) = owner {
      substate.record_storage_occupy(&address, COLLATERAL_UNITS_PER_STORAGE_KEY);
    }
    self.insert_storage_write_cache(key, StorageEntry::with_owner(value, owner));
  }

  fn original_storage_entry_at(&self, key: &[u8]) -> StorageEntry {
    if let Some(entry) = self.storage_committed_cache.read().get(key) {
      return *entry;
    }
    if self.fresh_storage() && !self.storage_overrided {
      return StorageEntry::default();
    }
    if let Some(entry) = self.storage_read_cache.read().get(key) {
      return *entry;
    }
    if self.storage_overrided {
      return StorageEntry::default();
    }
    panic!("storage must be read before recording its first transaction write")
  }

  pub fn delete_storage_range(
    &mut self,
    db: &mut dyn Database,
    key_prefix: &[u8],
    substate: &mut Substate,
  ) -> DbResult<()> {
    assert_eq!(self.address.space, Space::Native);
    let delete_all = key_prefix.is_empty();
    assert!(self.storage_write_checkpoint.is_none());
    let write_cache = &mut self.storage_write_cache.write();
    let commit_cache = &mut self.storage_committed_cache.write();
    let mut read_keys = HashSet::new();
    for (key, item) in write_cache.iter_mut() {
      let WriteCacheItem::Write(entry) = item else {
        read_keys.insert(key.clone());
        continue;
      };
      if key.starts_with(key_prefix) && !entry.value.is_zero() {
        if let Some(owner) = entry.refundable_owner() {
          substate.record_storage_release(&owner, COLLATERAL_UNITS_PER_STORAGE_KEY);
        }
        *entry = StorageEntry::default();
      }
    }
    for key in read_keys {
      write_cache.remove(&key);
    }
    for (key, entry) in commit_cache.iter() {
      if write_cache.contains_key(key) {
        continue;
      }
      if key.starts_with(key_prefix) && !entry.value.is_zero() {
        if let Some(owner) = entry.refundable_owner() {
          substate.record_storage_release(&owner, COLLATERAL_UNITS_PER_STORAGE_KEY);
        }
        write_cache.insert(key.clone(), WriteCacheItem::Write(StorageEntry::default()));
      }
    }
    let read_cache = self.storage_read_cache.read();
    // A newly created ordinary contract has no inherited storage to enumerate.
    // Builtins and overrides do not establish that same lifecycle fact.
    if !(self.is_newly_created_contract && !self.address.address.is_builtin_address()) {
      db.visit_storage(&self.address, key_prefix, &mut |key, load| {
        match write_cache.entry(key.to_vec()) {
          Occupied(_) => return Ok(()),
          Vacant(entry) => {
            if !delete_all {
              entry.insert(WriteCacheItem::Write(StorageEntry::default()));
            }
            // Preserve the protocol's legacy whitelist refund behavior.
            if !delete_all && !read_cache.contains_key(key) {
              return Ok(());
            }
          }
        }
        let entry = load()?;
        assert!(
          !entry.value.is_zero(),
          "persisted storage enumeration contains a zero entry"
        );
        if let Some(owner) = entry.refundable_owner() {
          substate.record_storage_release(&owner, COLLATERAL_UNITS_PER_STORAGE_KEY);
        }
        Ok(())
      })?;
    }
    drop(read_cache);
    if delete_all {
      write_cache.clear();
      self.storage_read_cache.write().clear();
      self.pending_db_clear = true;
    }
    Ok(())
  }

  fn cached_entry_at(&self, key: &[u8]) -> Option<StorageEntry> {
    if let Some(WriteCacheItem::Write(entry)) = self.storage_write_cache.read().get(key) {
      return Some(*entry);
    }
    if let Some(entry) = self.storage_committed_cache.read().get(key) {
      return Some(*entry);
    }
    if let Some(entry) = self.storage_read_cache.read().get(key) {
      return Some(*entry);
    }
    None
  }

  // If a contract is removed, and then some one transfer balance to it,
  // `storage_at` will return incorrect value. But this case should never
  // happens.
  pub fn storage_at(&self, db: &dyn Database, key: &[u8]) -> DbResult<U256> {
    Ok(self.storage_entry_at(db, key)?.value)
  }

  pub fn origin_storage_at(&self, key: &[u8]) -> Option<U256> {
    if let Some(value) = self.storage_committed_cache.read().get(key) {
      return Some(value.value);
    }
    if self.fresh_storage() && !self.storage_overrided {
      return Some(U256::zero());
    }
    if let Some(value) = self.storage_read_cache.read().get(key) {
      return Some(value.value);
    }
    if self.storage_overrided {
      return Some(U256::zero());
    }
    None
  }

  // If a contract is removed, and then some one transfer balance to it,
  // `storage_at` will return incorrect value. But this case should never
  // happens.
  pub fn storage_entry_at(&self, db: &dyn Database, key: &[u8]) -> DbResult<StorageEntry> {
    self.mark_storage_warm(key);
    Ok(if let Some(value) = self.cached_entry_at(key) {
      value
    } else if self.fresh_storage() {
      StorageEntry::default()
    } else {
      self.get_and_cache_storage(db, key)?
    })
  }

  pub fn transient_storage_at(&self, key: &[u8]) -> U256 {
    self
      .transient_storage_cache
      .read()
      .get(key)
      .cloned()
      .unwrap_or_default()
  }

  fn get_and_cache_storage(&self, db: &dyn Database, key: &[u8]) -> DbResult<StorageEntry> {
    let entry = db
      .get_storage_entry(&self.address, key)?
      .unwrap_or_default();
    self.storage_read_cache.write().insert(key.to_vec(), entry);
    Ok(entry)
  }

  pub fn transient_set_storage(&mut self, key: Vec<u8>, value: U256) {
    self.insert_transient_write_cache(key, value);
  }

  pub(super) fn should_have_owner(&self, _key: &[u8]) -> bool {
    self.address.space == Space::Native && self.address.address != SYSTEM_STORAGE_ADDRESS
  }

  pub fn change_storage_value(
    &mut self,
    db: &dyn Database,
    key: &[u8],
    value: U256,
  ) -> DbResult<()> {
    let mut entry = self.storage_entry_at(db, key)?;
    if !entry.value.is_zero() {
      entry = StorageEntry::with_owner(value, entry.owner);
      self.insert_storage_write_cache(key.to_vec(), entry);
    } else {
      warn!(
        "Change storage value outside transaction fails: current value is zero, tx {:?}, key {:?}",
        self.address, key
      );
    }
    Ok(())
  }

  pub fn is_warm_storage_entry(&self, key: &[u8]) -> bool {
    self.storage_write_cache.read().get(key).is_some()
  }
}
