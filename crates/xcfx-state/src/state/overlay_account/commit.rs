use crate::database::Database;
use crate::database::WriteCacheItem;
use std::sync::Arc;

use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_statedb::Result as DbResult;
use cfx_types::{address_util::AddressUtil, Space};

use super::OverlayAccount;

impl OverlayAccount {
  pub fn commit(
    self,
    db: &mut dyn Database,
    mut debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    assert!(self.storage_write_checkpoint.is_none());
    assert_eq!(Arc::strong_count(&self.storage_write_cache), 1);
    assert!(self.storage_write_cache.read().is_empty());
    if self.is_newly_created_contract
      && !(self.address.space == Space::Native && self.address.address.is_builtin_address())
    {
      db.record_contract_creation(&self.address);
    }
    for (key, entry) in self.storage_committed_cache.write().drain() {
      db.set_storage_entry(&self.address, &key, &entry, debug_record.as_deref_mut())?;
    }
    if let Some(code) = &self.code {
      db.set_code(
        &self.address,
        &self.code_hash,
        code,
        debug_record.as_deref_mut(),
      )?;
    }
    if let Some(list) = &self.deposit_list {
      self.address.assert_native();
      db.set_deposit_list(&self.address, list, debug_record.as_deref_mut())?;
    }
    if let Some(list) = &self.vote_stake_list {
      self.address.assert_native();
      db.set_vote_list(&self.address, list, debug_record.as_deref_mut())?;
    }
    if let Some(layout) = self.storage_layout_change.clone() {
      db.set_storage_layout(&self.address, layout, debug_record.as_deref_mut())?;
    }
    db.set_account(&self.as_account(), debug_record)
  }

  pub fn commit_cache(&mut self, retain_transient_storage: bool, dirty: bool) {
    assert!(self.storage_write_checkpoint.is_none());
    assert!(self.transient_storage_checkpoint.is_none());
    assert_eq!(Arc::strong_count(&self.storage_write_cache), 1);

    let mut storage_write_cache = self.storage_write_cache.write();
    let mut storage_commit_cache = self.storage_committed_cache.write();

    for (key, value) in storage_write_cache.drain() {
      if let WriteCacheItem::Write(value) = value {
        assert!(dirty);
        storage_commit_cache.insert(key, value);
      }
    }

    if !retain_transient_storage {
      self.transient_storage_cache.write().clear();
    }
  }
}
