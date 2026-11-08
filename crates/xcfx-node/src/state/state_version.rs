//! Immutable execution state backed by a local MPT or a fixed remote base.

use std::sync::Arc;

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_statedb::global_params::TOTAL_GLOBAL_PARAMS;
use cfx_types::U256;
use primitives::EpochId;
use xcfx_state::database::MptDatabase;
use xcfx_state::{State, database::Database};

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
  pub(crate) fn open_database(self: &Arc<Self>) -> Box<dyn Database> {
    match self.as_ref() {
      Self::Mpt(version) => Box::new(version.open_database()),
      Self::Fork(version) => Box::new(ForkStateCandidate::new(Arc::clone(version))),
    }
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

  /// Borrows this private candidate for execution and subsequent writeback.
  pub(crate) fn open_state(&mut self) -> cfx_statedb::Result<State<'_>> {
    match self {
      Self::Mpt(candidate) => State::from_database(Box::new(MptDatabase::new(candidate))),
      Self::Fork(candidate) => State::borrow_database(candidate),
    }
  }
}
