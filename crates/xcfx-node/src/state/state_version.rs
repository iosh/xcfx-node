//! Immutable execution state backed by a local MPT or a fixed remote base.

use std::sync::Arc;

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_statedb::{StateDb, global_params::TOTAL_GLOBAL_PARAMS};
use cfx_storage_types::{AccountClearMode, MptKeyValue, Result as StorageResult, StateStorage};
use cfx_types::{AddressWithSpace, U256};
use primitives::{EpochId, StorageKeyWithSpace};

use crate::fork::ForkClient;

use super::{
  fork_state::{ForkStateCandidate, ForkStateVersion},
  mpt_state::{MptStateCandidate, MptStateVersion},
};

pub(crate) enum StateVersion {
  Mpt(Arc<MptStateVersion>),
  Fork(Arc<ForkStateVersion>),
}

impl StateVersion {
  pub(crate) fn genesis_parent() -> Self {
    Self::Mpt(Arc::new(MptStateVersion::genesis_parent()))
  }

  /// Retains required initial data independently of the remote read cache.
  pub(crate) fn from_fork(client: ForkClient, globals: [U256; TOTAL_GLOBAL_PARAMS]) -> Self {
    Self::Fork(Arc::new(ForkStateVersion::new(client, globals)))
  }

  /// Opens a private candidate for a read or temporary execution.
  pub(crate) fn open_database(self: &Arc<Self>) -> StateDb<'static> {
    StateDb::from_owned(Box::new(StateCandidate::new(Arc::clone(self))))
  }

  /// Only a complete local MPT can supply these storage commitments.
  pub(crate) fn root_with_aux_info(&self) -> Option<StateRootWithAuxInfo> {
    match self {
      Self::Mpt(version) => Some(version.root_with_aux_info()),
      Self::Fork(_) => None,
    }
  }
}

/// One immutable state version bound to its committed epoch identity.
#[derive(Clone)]
pub(crate) struct CommittedStateVersion {
  pub(crate) epoch_id: EpochId,
  pub(crate) version: Arc<StateVersion>,
}

/// Unpublished changes whose backend kind is fixed by their parent.
pub(crate) enum StateCandidate {
  Mpt(MptStateCandidate),
  Fork(ForkStateCandidate),
}

impl StateCandidate {
  pub(crate) fn new(parent: Arc<StateVersion>) -> Self {
    match parent.as_ref() {
      StateVersion::Mpt(version) => Self::Mpt(MptStateCandidate::new(Arc::clone(version))),
      StateVersion::Fork(version) => Self::Fork(ForkStateCandidate::new(Arc::clone(version))),
    }
  }

  pub(crate) fn into_version(self) -> StateVersion {
    match self {
      Self::Mpt(candidate) => StateVersion::Mpt(Arc::new(candidate.into_version())),
      Self::Fork(candidate) => StateVersion::Fork(Arc::new(candidate.into_version())),
    }
  }

  fn storage(&self) -> &dyn StateStorage {
    match self {
      Self::Mpt(candidate) => candidate,
      Self::Fork(candidate) => candidate,
    }
  }

  fn storage_mut(&mut self) -> &mut dyn StateStorage {
    match self {
      Self::Mpt(candidate) => candidate,
      Self::Fork(candidate) => candidate,
    }
  }
}

impl StateStorage for StateCandidate {
  fn get_account_balance(&self, address: &AddressWithSpace) -> StorageResult<U256> {
    self.storage().get_account_balance(address)
  }

  fn account_clear_mode(&self) -> AccountClearMode {
    self.storage().account_clear_mode()
  }

  fn get(&self, key: StorageKeyWithSpace<'_>) -> StorageResult<Option<Box<[u8]>>> {
    self.storage().get(key)
  }

  fn set(&mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>) -> StorageResult<()> {
    self.storage_mut().set(key, value)
  }

  fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> StorageResult<()> {
    self.storage_mut().delete(key)
  }

  fn delete_test_only(&mut self, key: StorageKeyWithSpace<'_>) -> StorageResult<Option<Box<[u8]>>> {
    self.storage_mut().delete_test_only(key)
  }

  fn delete_all(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
  ) -> StorageResult<Option<Vec<MptKeyValue>>> {
    self.storage_mut().delete_all(prefix)
  }

  fn clear_account_storage(&mut self, address: &AddressWithSpace) -> StorageResult<()> {
    self.storage_mut().clear_account_storage(address)
  }

  fn clear_account_code(&mut self, address: &AddressWithSpace) -> StorageResult<()> {
    self.storage_mut().clear_account_code(address)
  }

  fn read_all(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
  ) -> StorageResult<Option<Vec<MptKeyValue>>> {
    self.storage_mut().read_all(prefix)
  }

  fn read_all_with_callback(
    &mut self,
    prefix: StorageKeyWithSpace<'_>,
    callback: &mut dyn FnMut(MptKeyValue),
    only_account_key: bool,
  ) -> StorageResult<()> {
    self
      .storage_mut()
      .read_all_with_callback(prefix, callback, only_account_key)
  }
}
