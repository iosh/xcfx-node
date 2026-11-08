use crate::state::overlay_account::AccountEntry;
use crate::AccountNonce;

use super::State;
use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_statedb::Result as DbResult;
use cfx_types::AddressWithSpace;

impl State<'_> {
  /// Writes account and global changes to the private backend,
  /// returning the nonce updates used for transaction-pool reconciliation.
  ///
  /// This does not compute a state root or commit an epoch. Release the
  /// State before finalizing storage borrowed from the caller.
  ///
  /// # Errors
  ///
  /// Returns a database error if a change cannot be applied. The backend may
  /// contain partial writes, so the caller must abandon the working state
  /// rather than publish it.
  ///
  /// # Panics
  ///
  /// Panics if a checkpoint is still active.
  pub fn apply_changes_to_storage(
    &mut self,
    mut debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<Vec<AccountNonce>> {
    let accounts_for_txpool = self.write_records(debug_record.as_deref_mut())?;
    self.db.flush(debug_record)?;
    Ok(accounts_for_txpool)
  }

  /// Writes account records and global parameters to the backing database.
  fn write_records(
    &mut self,
    mut debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<Vec<AccountNonce>> {
    debug!("state.commit_changes");

    let accounts_for_txpool = self.commit_dirty_accounts(debug_record.as_deref_mut())?;
    self.global_stat.commit(&mut *self.db, debug_record)?;
    Ok(accounts_for_txpool)
  }

  fn commit_dirty_accounts(
    &mut self,
    mut debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<Vec<AccountNonce>> {
    assert!(self.no_checkpoint());

    self.commit_cache(false);
    let cache_items = self.committed_cache.drain();
    let mut to_commit_accounts = cache_items
      .filter_map(|(_, acc)| acc.into_to_commit_account())
      .collect::<Vec<_>>();
    to_commit_accounts.sort_by(|a, b| a.address().cmp(b.address()));

    let mut accounts_to_notify = vec![];

    for mut account in to_commit_accounts.into_iter() {
      let address = *account.address();
      if account.as_account().default_status().is_none() {
        let initialized = self.db.storage_points_initialized(&address.address)?;
        account.resolve_storage_points_flag(initialized);
      }

      if account.pending_db_clear() {
        self.recycle_storage(vec![address], debug_record.as_deref_mut())?;
      }

      if !account.removed_without_update() {
        accounts_to_notify.push(AccountNonce {
          address,
          nonce: *account.nonce(),
        });
        account.commit(&mut *self.db, debug_record.as_deref_mut())?;
      }
    }
    Ok(accounts_to_notify)
  }

  fn recycle_storage(
    &mut self,
    killed_addresses: Vec<AddressWithSpace>,
    mut debug_record: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()> {
    for address in killed_addresses {
      self
        .db
        .delete_account(&address, debug_record.as_deref_mut())?;
    }
    Ok(())
  }
}

impl State<'_> {
  pub fn commit_cache(&mut self, retain_transient_storage: bool) {
    assert!(self.no_checkpoint());
    for (addr, mut account) in self.cache.get_mut().drain() {
      if let AccountEntry::Cached(ref mut acc, dirty) = account.entry {
        acc.commit_cache(retain_transient_storage, dirty);
      }
      self.committed_cache.insert(addr, account.entry);
    }
  }
}
