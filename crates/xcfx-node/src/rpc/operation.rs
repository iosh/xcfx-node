use std::{future::Future, pin::Pin};

use serde_json::Value;
use tokio::sync::{oneshot, watch};

use crate::node::{NodeCall, NodeHandle, NodeTask};

use super::{connection::ConnectionCall, response::RpcError};

pub(super) type RpcResult = Result<Value, RpcError>;

pub(super) enum Operation {
  /// Only results independent of execution or connection observation order.
  Immediate(Value),
  OnNode(NodeTask<ResponseConversion>),
  OnConnection(ConnectionTask),
}

pub(super) type ConnectionTask = Box<dyn FnOnce(ConnectionCall<'_>) -> RpcResult + Send>;

/// Owns a node result and its conversion, which runs only on IO. A successful
/// operation can require further waiting; an execution error can finish directly.
/// Converters must not capture mutable protocol state: their panic boundary is
/// request-local. Ordered connection mutations use OnConnection instead.
pub(super) type ResponseConversion = Box<dyn FnOnce() -> ResponseCompletion + Send>;

pub(super) enum ResponseCompletion {
  Ready(RpcResult),
  Pending(PendingResponse),
}

pub(super) enum PendingResponse {
  Wait(Box<dyn FnOnce(CompletionContext) -> ResponseFuture + Send>),
  /// A started observation must be applied even if cancellation arrives later.
  Observation(oneshot::Receiver<RpcResult>),
}

type ResponseFuture = Pin<Box<dyn Future<Output = RpcResult> + Send>>;

/// Created before the completion starts its first query, so a fact arriving
/// during that query leaves an unread wakeup. Wakeups do not retain chain history.
pub(super) struct CompletionContext {
  node: NodeHandle,
  changes: watch::Receiver<()>,
}

impl CompletionContext {
  pub(super) fn new(node: NodeHandle, changes: watch::Receiver<()>) -> Self {
    Self { node, changes }
  }

  pub(super) fn node(&self) -> &NodeHandle {
    &self.node
  }

  pub(super) async fn changed(&mut self) {
    self
      .changes
      .changed()
      .await
      .expect("the change publisher outlives response completion tasks");
  }
}

impl ResponseCompletion {
  pub(super) fn wait<Fut: Future<Output = RpcResult> + Send + 'static>(
    complete: impl FnOnce(CompletionContext) -> Fut + Send + 'static,
  ) -> Self {
    Self::Pending(PendingResponse::Wait(Box::new(move |context| {
      Box::pin(complete(context))
    })))
  }
}

impl Operation {
  pub(super) fn on_node<T: Send + 'static>(
    execute: impl FnOnce(NodeCall<'_, '_>) -> T + Send + 'static,
    convert: impl FnOnce(T) -> RpcResult + Send + 'static,
  ) -> Self {
    Self::on_node_with_completion(execute, move |value| {
      ResponseCompletion::Ready(convert(value))
    })
  }

  pub(super) fn on_node_with_completion<T: Send + 'static>(
    execute: impl FnOnce(NodeCall<'_, '_>) -> T + Send + 'static,
    complete: impl FnOnce(T) -> ResponseCompletion + Send + 'static,
  ) -> Self {
    Self::OnNode(Box::new(move |node| {
      let value = execute(node);
      Box::new(move || complete(value))
    }))
  }

  pub(super) fn on_connection(
    execute: impl FnOnce(ConnectionCall<'_>) -> RpcResult + Send + 'static,
  ) -> Self {
    Self::OnConnection(Box::new(execute))
  }
}
