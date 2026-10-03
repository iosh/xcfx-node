//! Transport-independent operations, results, and cancellation signals.

use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use cfx_types::{AddressWithSpace, H256, U256};
use thiserror::Error;
use tokio::sync::{Notify, oneshot};

use crate::{
  chain::TransactionReceiptView,
  config::MiningMode,
  production_environment::BlockTimestamp,
  runtime::{EpochSelector, RuntimeBlockProductionError, RuntimeTransactionError, StateQueryError},
  state_overlay::{StateControlBatch, StateControlPreparationError},
};

use super::NodeError;
use super::mining::Tick;

/// Scoped to one node's command stream, independently of JSON-RPC IDs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MessageId(pub(super) u64);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CallId {
  pub(crate) message: MessageId,
  pub(crate) index: usize,
}

/// A one-way signal. Dropping a clone does not cancel other users' work.
#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<CancellationState>);

#[derive(Default)]
struct CancellationState {
  cancelled: AtomicBool,
  changed: Notify,
}

impl Cancellation {
  pub(crate) fn cancel(&self) {
    if !self.0.cancelled.swap(true, Ordering::AcqRel) {
      self.0.changed.notify_waiters();
    }
  }

  pub(crate) fn is_cancelled(&self) -> bool {
    self.0.cancelled.load(Ordering::Acquire)
  }

  pub(crate) async fn cancelled(&self) {
    loop {
      let changed = self.0.changed.notified();
      tokio::pin!(changed);
      changed.as_mut().enable();
      if self.is_cancelled() {
        return;
      }
      changed.await;
    }
  }
}

pub(crate) enum NodeCommand {
  EpochHeight(EpochSelector),
  Balance {
    address: AddressWithSpace,
    epoch: EpochSelector,
  },
  /// Looks up a local receipt under the Runtime's deferred visibility rules.
  TransactionReceipt(H256),
  SubmitRawTransaction(Vec<u8>),
  ApplyStateControls(StateControlBatch),
  Mine(MineRequest),
  MiningMode,
  SetAutomine(bool),
  /// Zero disables interval mining; positive values select interval mode.
  SetIntervalMining(Duration),
}

pub(crate) enum CommandOutcome {
  EpochHeight(u64),
  Balance(U256),
  TransactionReceipt(Option<TransactionReceiptView>),
  TransactionSubmitted(H256),
  StateControlsApplied,
  Mined(MiningOutcome),
  MiningMode(MiningMode),
}

#[derive(Debug, Error)]
pub(crate) enum CommandError {
  #[error(transparent)]
  Request(#[from] RequestError),
  #[error(transparent)]
  Transaction(#[from] RuntimeTransactionError),
  #[error(transparent)]
  StateControl(#[from] StateControlPreparationError),
  #[error(transparent)]
  StateQuery(#[from] StateQueryError),
  #[error(transparent)]
  MiningInterval(#[from] InvalidMiningInterval),
}

#[derive(Clone, Debug, Error)]
pub(crate) enum RequestError {
  #[error("request was cancelled; completed changes are not rolled back")]
  Cancelled,
  #[error(transparent)]
  Node(#[from] NodeError),
}

pub(crate) struct MessageOutcome {
  pub(crate) id: MessageId,
  /// Results retain input order, including skipped or failed calls.
  pub(crate) calls: Vec<Result<CommandOutcome, CommandError>>,
}

pub(super) struct Call {
  pub(super) command: NodeCommand,
  pub(super) cancellation: Cancellation,
}

pub(super) struct Message {
  pub(super) id: MessageId,
  pub(super) calls: Vec<Call>,
  pub(super) cancellation: Cancellation,
  pub(super) reply: oneshot::Sender<MessageOutcome>,
}

pub(super) enum Work {
  Message(Message),
  Tick(Tick),
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum MineTiming {
  #[default]
  Default,
  /// A logical timestamp increment for each block, not a wall-clock delay.
  Interval(u64),
  /// Overrides the first block's timestamp only.
  Timestamp(u64),
}

impl MineTiming {
  pub(super) fn for_block(self, index: u64) -> BlockTimestamp {
    match self {
      Self::Default => BlockTimestamp::Default,
      Self::Interval(increment) => BlockTimestamp::Increment(increment),
      Self::Timestamp(timestamp) if index == 0 => BlockTimestamp::Exact(timestamp),
      Self::Timestamp(_) => BlockTimestamp::Default,
    }
  }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MineRequest {
  pub(crate) blocks: u64,
  pub(crate) timing: MineTiming,
}

impl Default for MineRequest {
  fn default() -> Self {
    Self {
      blocks: 1,
      timing: MineTiming::Default,
    }
  }
}

pub(crate) enum MiningOutcome {
  Complete(Vec<H256>),
  Interrupted {
    hashes: Vec<H256>,
    reason: MiningError,
  },
}

#[derive(Debug, Error)]
pub(crate) enum MiningError {
  #[error(transparent)]
  Production(#[from] RuntimeBlockProductionError),
  #[error(transparent)]
  Request(#[from] RequestError),
  #[error(transparent)]
  Interval(#[from] InvalidMiningInterval),
}

#[derive(Clone, Copy, Debug, Error)]
#[error("mining interval is outside the monotonic clock's supported range")]
pub(crate) struct InvalidMiningInterval;
