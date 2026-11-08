//! Typed data access; execution and collateral rules belong to State.
mod mpt;
mod records;
pub use mpt::MptDatabase;
pub(crate) use records::WriteCacheItem;
pub use records::{CodeEntry, StorageEntry, StorageOwner};

use crate::account::Account;
use cfx_internal_common::debug::ComputeEpochDebugRecord;
use cfx_statedb::Result as DbResult;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, U256};
use primitives::{DepositList, StorageKeyWithSpace, StorageLayout, VoteStakeList};
use std::{
  io,
  ops::{Deref, DerefMut},
};

/// Allows a caller to skip decoding an entry shadowed by its current writes.
pub type StorageVisitor<'a> =
  dyn FnMut(&[u8], &dyn Fn() -> DbResult<StorageEntry>) -> DbResult<()> + 'a;

/// Supplies records from one backing version and receives private writes.
///
/// A failed read or flush must abort the enclosing candidate. This interface
/// neither publishes a version nor chooses collateral charges or refunds.
pub trait Database {
  fn is_initialized(&self) -> DbResult<bool>;
  fn get_account(&self, address: &AddressWithSpace) -> DbResult<Option<Account>>;
  fn get_account_balance(&self, address: &AddressWithSpace) -> DbResult<U256>;
  fn get_code(&self, address: &AddressWithSpace, hash: &H256) -> DbResult<Option<CodeEntry>>;
  fn get_storage_entry(
    &self,
    address: &AddressWithSpace,
    key: &[u8],
  ) -> DbResult<Option<StorageEntry>>;
  fn get_deposit_list(&self, address: &AddressWithSpace) -> DbResult<Option<DepositList>>;
  fn get_vote_list(&self, address: &AddressWithSpace) -> DbResult<Option<VoteStakeList>>;
  fn get_global_param(&self, key: StorageKeyWithSpace<'_>) -> DbResult<Option<U256>>;
  fn visit_storage(
    &mut self,
    address: &AddressWithSpace,
    prefix: &[u8],
    visitor: &mut StorageVisitor<'_>,
  ) -> DbResult<()>;
  /// Records a new ordinary contract before any of its records are written.
  ///
  /// Partial backends must stop inheriting its previous code, storage, deposit
  /// and vote records. Complete backends need no additional creation marker.
  fn record_contract_creation(&mut self, address: &AddressWithSpace);
  fn set_account(
    &mut self,
    account: &Account,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_code(
    &mut self,
    address: &AddressWithSpace,
    hash: &H256,
    code: &CodeEntry,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_storage_entry(
    &mut self,
    address: &AddressWithSpace,
    key: &[u8],
    entry: &StorageEntry,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_deposit_list(
    &mut self,
    address: &AddressWithSpace,
    list: &DepositList,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_vote_list(
    &mut self,
    address: &AddressWithSpace,
    list: &VoteStakeList,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_global_param(
    &mut self,
    key: StorageKeyWithSpace<'_>,
    value: &U256,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn set_storage_layout(
    &mut self,
    address: &AddressWithSpace,
    layout: StorageLayout,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  /// Deletes the account, code, storage/layout, deposit and vote records.
  fn delete_account(
    &mut self,
    address: &AddressWithSpace,
    debug: Option<&mut ComputeEpochDebugRecord>,
  ) -> DbResult<()>;
  fn flush(&mut self, debug: Option<&mut ComputeEpochDebugRecord>) -> DbResult<()>;

  /// Resolves an inherited initialization flag without replacing local amounts.
  /// Backends with partial account records must override this when they can
  /// obtain the missing flag from their fixed source.
  fn storage_points_initialized(&self, address: &Address) -> DbResult<bool> {
    let Some(account) = self.get_account(&address.with_native_space())? else {
      return Ok(false);
    };
    account
      .sponsor_info
      .storage_points
      .initialization()
      .ok_or_else(|| {
        cfx_storage_types::Error::Io(io::Error::new(
          io::ErrorKind::Unsupported,
          format!("storage-point initialization is unavailable for {address:?}"),
        ))
        .into()
      })
  }
}

pub(crate) enum DatabaseHandle<'db> {
  Owned(Box<dyn Database + 'db>),
  Borrowed(&'db mut dyn Database),
}

impl<'db> Deref for DatabaseHandle<'db> {
  type Target = dyn Database + 'db;
  fn deref(&self) -> &Self::Target {
    match self {
      Self::Owned(db) => &**db,
      Self::Borrowed(db) => &**db,
    }
  }
}
impl DerefMut for DatabaseHandle<'_> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    match self {
      Self::Owned(db) => &mut **db,
      Self::Borrowed(db) => &mut **db,
    }
  }
}
