//! A caching and checkpoint layer built upon semantically meaningful database
//! interfaces, providing interfaces and logics for managing accounts and global
//! statistics to the execution engine.

use crate::database::{Database, DatabaseHandle};

/// Contract Manager: Responsible for creating and deleting contract objects.
mod contract_manager;

/// Implements access functions for the basic fields (e.g., balance, nonce) of
/// `State`.
mod basic_fields;

/// Cache Layer: Implements a read-through write-back cache logic and provides
/// interfaces for reading and writing account data. It also handles the logic
/// for loading extension fields of an account.
mod cache_layer;

/// Checkpoints: Defines the account entry type within checkpoint layers and
/// implements checkpoint maintenance logic.
mod checkpoints;

/// Implements functions for the storage collateral of `State`.
mod collateral;

/// Implements functions for committing `State` changes to db.
mod commit;

/// Implements access functions global statistic variables of `State`.
mod global_statistics;

mod warm;

/// Implements functions for the PoS rewarding of `State`.
mod pos;

mod save;

/// Implements functions for the sponsorship mechanism of `State`.
mod sponsor;

/// Implements functions for the staking mechanism of `State`.
mod staking;

/// Implements access functions for the account storage entries of `State`.
mod storage_entry;

mod reward;

mod state_override;

pub use self::{
  collateral::{initialize_cip107, settle_collateral_for_all},
  pos::{distribute_pos_interest, update_pos_status},
  reward::initialize_cip137,
  sponsor::COMMISSION_PRIVILEGE_SPECIAL_KEY,
  staking::initialize_or_update_dao_voted_params,
};

use self::checkpoints::CheckpointLayer;
use super::{
  checkpoints::LazyDiscardedVec,
  global_stat::GlobalStat,
  overlay_account::{AccountEntry, AccountEntryWithWarm, OverlayAccount, RequireFields},
};
use crate::substate::Substate;
use cfx_statedb::Result as DbResult;
use cfx_types::{AddressWithSpace, H256};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

/// A caching and checkpoint layer built upon semantically meaningful database
/// interfaces, providing interfaces and logics for managing accounts and global
/// statistics to the execution engine.
pub struct State<'db> {
  /// The backend database
  pub(super) db: DatabaseHandle<'db>,

  /// Caches for the account entries
  ///
  /// WARNING: Don't delete cache entries outside of state writeback, unless
  /// you are familiar with checkpoint maintenance.
  pub(crate) cache: RwLock<HashMap<AddressWithSpace, AccountEntryWithWarm>>,

  pub(crate) committed_cache: HashMap<AddressWithSpace, AccountEntry>,
  tx_access_list: Option<HashMap<AddressWithSpace, HashSet<H256>>>,

  /// In-memory global statistic variables.
  // TODO: try not to make it special?
  global_stat: GlobalStat,

  /// Checkpoint layers for the account entries
  // TODO: it seems `RwLock` is not necessary here. But we need to change the
  // signature of `write_account` from `&self` to `&mut self` first
  checkpoints: RwLock<LazyDiscardedVec<CheckpointLayer>>,
}

impl<'db> State<'db> {
  pub fn from_database(db: Box<dyn Database + 'db>) -> DbResult<Self> {
    Self::with_database_handle(DatabaseHandle::Owned(db))
  }

  /// Borrows a private data candidate. It must outlive execution and writeback.
  pub fn borrow_database(db: &'db mut dyn Database) -> DbResult<Self> {
    Self::with_database_handle(DatabaseHandle::Borrowed(db))
  }

  fn with_database_handle(db: DatabaseHandle<'db>) -> DbResult<Self> {
    let global_stat = if db.is_initialized()? {
      GlobalStat::loaded(&*db)?
    } else {
      GlobalStat::assert_non_inited(&*db)?;
      GlobalStat::new()
    };
    Ok(Self {
      db,
      cache: Default::default(),
      committed_cache: Default::default(),
      checkpoints: Default::default(),
      tx_access_list: None,
      global_stat,
    })
  }
}
