//! Facts and observation boundaries in the execution thread's publication order.

use std::sync::Arc;

use crate::runtime::RuntimeCommitOutcome;

use super::{CallId, MiningCause, MiningError};

/// One stream orders connection observation changes against committed facts.
/// The consumer must drain it independently of response and notification output.
pub(crate) enum NodeEvent {
  Committed {
    cause: MiningCause,
    outcome: Arc<RuntimeCommitOutcome>,
  },
  MiningFailed {
    cause: MiningCause,
    error: Arc<MiningError>,
  },
  Observation {
    call: CallId,
  },
}
