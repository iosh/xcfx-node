//! Typed execution batches, request identities, and cancellation signals.

use std::sync::{
  Arc,
  atomic::{AtomicBool, Ordering},
};
use thiserror::Error;
use tokio::sync::{Notify, oneshot};

use super::{
  NodeError,
  mining::Tick,
  worker::{NodeCall, Worker},
};

/// Scoped to one node's execution stream, independently of JSON-RPC IDs.
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

/// A synchronous task may consume one scoped node operation and return an owned value.
pub(crate) type NodeTask<T> = Box<dyn FnOnce(NodeCall<'_, '_>) -> T + Send>;

/// An outer error means the batch could not be started or delivered. Once started,
/// every task has a result in input order; operation-specific errors remain in `T`.
pub(crate) type BatchResult<T> = Result<Vec<Result<T, RequestError>>, RequestError>;

#[derive(Clone, Debug, Error)]
pub(crate) enum RequestError {
  #[error("request was cancelled; completed changes are not rolled back")]
  Cancelled,
  #[error(transparent)]
  Node(#[from] NodeError),
}

pub(super) struct Call<T> {
  pub(super) index: usize,
  pub(super) task: NodeTask<T>,
  pub(super) cancellation: Cancellation,
}

pub(super) struct Message<T> {
  pub(super) id: MessageId,
  pub(super) calls: Vec<Call<T>>,
  pub(super) cancellation: Cancellation,
  /// Results are delivered together, after the message's automatic mining.
  pub(super) reply: oneshot::Sender<BatchResult<T>>,
}

/// Erases only the batch's return type at the queue boundary.
pub(super) trait QueuedMessage: Send {
  fn execute(self: Box<Self>, worker: &mut Worker<'_>);
  fn reject(self: Box<Self>, error: RequestError);
}

impl<T: Send> QueuedMessage for Message<T> {
  fn execute(self: Box<Self>, worker: &mut Worker<'_>) {
    worker.execute_message(*self);
  }

  fn reject(self: Box<Self>, error: RequestError) {
    let _ = self.reply.send(Err(error));
  }
}

pub(super) enum Work {
  Message(Box<dyn QueuedMessage>),
  Tick(Tick),
}
