//! Admission and shutdown state shared without sharing the Runtime.

use std::sync::{Mutex, MutexGuard};

use tokio::sync::{Notify, mpsc};

use super::{
  NodeError,
  request::{Cancellation, RequestError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NodeStatus {
  Starting,
  Ready,
  Stopping,
  Failed,
  Closed,
}

enum State {
  Starting,
  Ready,
  Stopping,
  Failed(NodeError),
  Closed(Result<(), NodeError>),
}

pub(super) struct Lifecycle {
  state: Mutex<State>,
  changed: Notify,
}

impl Lifecycle {
  pub(super) fn new() -> Self {
    Self {
      state: Mutex::new(State::Starting),
      changed: Notify::new(),
    }
  }

  fn state(&self) -> MutexGuard<'_, State> {
    // Only lifecycle metadata is protected here. Runtime panics cannot poison
    // this lock; recovering it also lets supervision finish an unexpected unwind.
    self.state.lock().unwrap_or_else(|error| error.into_inner())
  }

  pub(super) fn status(&self) -> NodeStatus {
    match &*self.state() {
      State::Starting => NodeStatus::Starting,
      State::Ready => NodeStatus::Ready,
      State::Stopping => NodeStatus::Stopping,
      State::Failed(_) => NodeStatus::Failed,
      State::Closed(_) => NodeStatus::Closed,
    }
  }

  pub(super) fn mark_ready(&self) -> bool {
    let mut state = self.state();
    if !matches!(*state, State::Starting) {
      return false;
    }
    *state = State::Ready;
    drop(state);
    self.changed.notify_waiters();
    true
  }

  pub(super) fn stop(&self) {
    let mut state = self.state();
    if matches!(*state, State::Starting | State::Ready) {
      *state = State::Stopping;
    }
    drop(state);
    self.changed.notify_waiters();
  }

  pub(super) fn fail(&self, error: NodeError) {
    let mut state = self.state();
    match (&*state, &error) {
      // Joining enriches a failure already detected at an execution boundary.
      (
        State::Failed(NodeError::ForkServiceStopped(None)),
        NodeError::ForkServiceStopped(Some(_)),
      ) => {
        *state = State::Failed(error);
      }
      (State::Failed(_) | State::Closed(_), _) => {}
      _ => *state = State::Failed(error),
    }
    drop(state);
    self.changed.notify_waiters();
  }

  pub(super) fn finish(&self, result: Result<(), NodeError>) {
    let mut state = self.state();
    let result = match &*state {
      State::Failed(error) => Err(error.clone()),
      State::Closed(_) => return,
      _ => result,
    };
    *state = State::Closed(result);
    drop(state);
    self.changed.notify_waiters();
  }

  fn rejection(state: &State) -> Option<NodeError> {
    match state {
      State::Ready => None,
      State::Failed(error) | State::Closed(Err(error)) => Some(error.clone()),
      State::Starting | State::Stopping | State::Closed(Ok(())) => Some(NodeError::Closed),
    }
  }

  /// Checks startup readiness under the same lock as stop and failure.
  pub(super) fn check_ready(&self) -> Result<(), NodeError> {
    Self::rejection(&self.state()).map_or(Ok(()), Err)
  }

  pub(super) fn unavailable(&self) -> NodeError {
    Self::rejection(&self.state()).unwrap_or(NodeError::ExecutionPanicked)
  }

  /// The admission check and publication share the same boundary as stop.
  pub(super) fn admit<T>(
    &self,
    permit: mpsc::Permit<'_, T>,
    value: T,
    cancellation: Option<&Cancellation>,
  ) -> Result<(), RequestError> {
    let state = self.state();
    if let Some(error) = Self::rejection(&state) {
      return Err(error.into());
    }
    if cancellation.is_some_and(Cancellation::is_cancelled) {
      return Err(RequestError::Cancelled);
    }
    permit.send(value);
    Ok(())
  }

  /// Returning success is the start boundary of one indivisible Runtime operation.
  pub(super) fn begin(&self, cancellations: &[&Cancellation]) -> Result<(), RequestError> {
    let state = self.state();
    if let Some(error) = Self::rejection(&state) {
      return Err(error.into());
    }
    if cancellations.iter().any(|signal| signal.is_cancelled()) {
      return Err(RequestError::Cancelled);
    }
    Ok(())
  }

  pub(super) async fn stopped(&self) {
    loop {
      let changed = self.changed.notified();
      tokio::pin!(changed);
      changed.as_mut().enable();
      if !matches!(*self.state(), State::Starting | State::Ready) {
        return;
      }
      changed.await;
    }
  }

  pub(super) async fn closed(&self) -> Result<(), NodeError> {
    loop {
      let changed = self.changed.notified();
      tokio::pin!(changed);
      changed.as_mut().enable();
      if let State::Closed(result) = &*self.state() {
        return result.clone();
      }
      changed.await;
    }
  }
}
