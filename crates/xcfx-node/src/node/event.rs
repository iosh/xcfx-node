//! Facts and observation boundaries in the execution thread's publication order.

use std::sync::Arc;

use crate::{
  logs::{LogFilterInput, LogQueryError},
  runtime::{ChainChange, RuntimeCommitOutcome},
  runtime_transaction::RuntimeTransaction,
};
use cfx_addr::Network;
use primitives::filter::LogFilterParams;

use super::{CallId, MiningCause, MiningError};

pub(crate) enum ObservationRequest {
  Chain,
  Logs(LogFilterInput),
}

pub(crate) enum Observation {
  Chain {
    network: Network,
    mined_epoch: u64,
    visible_epoch: u64,
  },
  Logs {
    network: Network,
    filter: LogFilterParams,
    visible_epoch: u64,
  },
}

/// One stream orders connection observation changes against committed facts.
/// The consumer must drain it independently of response and notification output.
pub(crate) enum NodeEvent {
  Committed {
    cause: MiningCause,
    outcome: Arc<RuntimeCommitOutcome>,
  },
  HistoryChanged {
    call: CallId,
    change: Arc<ChainChange>,
  },
  TransactionInserted {
    call: CallId,
    transaction: RuntimeTransaction,
  },
  MiningFailed {
    cause: MiningCause,
    error: Arc<MiningError>,
  },
  Observation {
    call: CallId,
    result: Result<Observation, LogQueryError>,
  },
}
