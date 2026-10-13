use std::{
  collections::HashMap,
  panic::{AssertUnwindSafe, catch_unwind},
};

use cfx_types::Space;
use serde_json::{Value, value::RawValue};
use tokio::sync::oneshot;

use crate::node::{NodeTask, RequestError};

use super::{
  connection::ConnectionCall,
  dev::DevRequest,
  espace::EspaceRequest,
  operation::{
    ConnectionTask, Operation, PendingResponse, ResponseCompletion, ResponseConversion, RpcResult,
  },
  request::{Call, Id, Request},
  response::{Response, RpcError, RpcResponse},
  serde_helpers::deserialize_request,
};

// Every input position has a slot, including notifications. Identity stays
// outside state transitions so preparation or conversion panics cannot lose it.
struct CallSlot {
  id: Option<Id>,
  state: CallState,
}

enum CallState {
  Unprepared {
    method: String,
    params: Option<Box<RawValue>>,
  },
  Pending,
  Waiting,
  Ready(Result<Value, RpcError>),
  NotificationComplete,
}

impl CallSlot {
  fn complete(&mut self, result: Result<Value, RpcError>) {
    // Notifications still complete and validate their outcomes, but retain no
    // response body while other calls in the message are being processed.
    self.state = if self.id.is_some() {
      CallState::Ready(result)
    } else {
      CallState::NotificationComplete
    };
  }

  fn is_complete(&self) -> bool {
    matches!(
      &self.state,
      CallState::Ready(_) | CallState::NotificationComplete
    )
  }
}

/// Retains call identities and results throughout dispatch of one JSON-RPC message.
pub(super) struct MessageDispatch {
  batch: bool,
  calls: Vec<CallSlot>,
  observations: HashMap<usize, Observation>,
}

struct Observation {
  apply: ConnectionTask,
  reply: oneshot::Sender<RpcResult>,
}

impl MessageDispatch {
  pub(super) fn new(request: Request) -> Self {
    let (batch, calls) = match request {
      Request::Single(call) => (false, vec![call]),
      Request::Batch(calls) => (true, calls),
    };
    Self {
      batch,
      observations: HashMap::new(),
      calls: calls
        .into_iter()
        .map(|call| match call {
          Call::Invalid { id } => CallSlot {
            id: Some(id),
            state: CallState::Ready(Err(RpcError::invalid_request())),
          },
          Call::Method(call) => CallSlot {
            id: call.id,
            state: CallState::Unprepared {
              method: call.method,
              params: call.params,
            },
          },
        })
        .collect(),
    }
  }

  pub(super) fn prepare(&mut self, space: Space) -> Vec<(usize, NodeTask<ResponseConversion>)> {
    let mut tasks = Vec::new();
    for (index, call) in self.calls.iter_mut().enumerate() {
      let CallState::Unprepared { method, params } = &mut call.state else {
        continue;
      };
      match prepare_method(space, method, params.as_deref()) {
        Ok(Operation::OnNode(task)) => {
          tasks.push((index, task));
          call.state = CallState::Pending;
        }
        Ok(Operation::OnConnection(apply)) => {
          let (reply, response) = oneshot::channel();
          self
            .observations
            .insert(index, Observation { apply, reply });
          let task: NodeTask<ResponseConversion> = Box::new(move |node| {
            node.observation_boundary();
            Box::new(move || ResponseCompletion::Pending(PendingResponse::Observation(response)))
          });
          tasks.push((index, task));
          call.state = CallState::Pending;
        }
        Ok(Operation::Immediate(value)) => call.complete(Ok(value)),
        Err(error) => call.complete(Err(error)),
      }
    }
    tasks
  }

  /// Runs conversions after the entire execution batch, including automatic
  /// mining. Only unfinished responses leave this function as asynchronous work.
  pub(super) fn complete(
    &mut self,
    results: Vec<Result<ResponseConversion, RequestError>>,
    mut on_panic: impl FnMut(usize),
  ) -> Vec<(usize, PendingResponse)> {
    let mut results = results.into_iter();
    let mut pending = Vec::new();
    for (index, call) in self.calls.iter_mut().enumerate() {
      if call.is_complete() {
        continue;
      }
      assert!(
        matches!(&call.state, CallState::Pending),
        "all RPC calls were prepared before submitting the message"
      );
      // Preparation and execution both preserve the order of pending slots.
      // Install the fallback before consuming that slot's one-shot conversion.
      call.complete(Err(RpcError::internal()));
      let result = results
        .next()
        .expect("every pending RPC call has a task result");
      let conversion = match result {
        Ok(conversion) => conversion,
        Err(error) => {
          // A skipped observation has no marker in the fact stream.
          self.observations.remove(&index);
          call.complete(Err(RpcError::from(&error)));
          continue;
        }
      };
      match catch_unwind(AssertUnwindSafe(conversion)) {
        Ok(ResponseCompletion::Ready(result)) => call.complete(result),
        Ok(ResponseCompletion::Pending(response)) => {
          call.state = CallState::Waiting;
          pending.push((index, response));
        }
        Err(_) => on_panic(index),
      }
    }
    assert!(
      results.next().is_none(),
      "every task result has an RPC call slot"
    );
    pending
  }

  pub(super) fn complete_waiting(&mut self, index: usize, result: RpcResult) {
    let call = &mut self.calls[index];
    assert!(
      matches!(&call.state, CallState::Waiting),
      "a completion task owns one waiting RPC slot"
    );
    call.complete(result);
  }

  /// Called in fact-stream order, independently of when the batch reply arrives.
  /// Retain the sender across the shared-state panic boundary so its waiter ends.
  pub(super) fn observe(
    &mut self,
    index: usize,
    connection: ConnectionCall<'_>,
    on_panic: impl FnOnce(),
  ) {
    let Observation { apply, reply } = self
      .observations
      .remove(&index)
      .expect("an observation marker identifies its registered operation");
    let result = catch_unwind(AssertUnwindSafe(|| apply(connection))).unwrap_or_else(|_| {
      on_panic();
      Err(RpcError::internal())
    });
    let _ = reply.send(result);
  }

  /// No further marker can arrive after the worker's fact stream is drained.
  pub(super) fn close_observations(&mut self) {
    self.observations.clear();
  }

  pub(super) fn into_response(self) -> Option<Response> {
    let mut responses: Vec<_> = self
      .calls
      .into_iter()
      .filter_map(|call| {
        let id = call.id?;
        let result = match call.state {
          CallState::Ready(result) => result,
          _ => Err(RpcError::internal()),
        };
        Some(RpcResponse::new(id, result))
      })
      .collect();
    if responses.is_empty() {
      None
    } else if self.batch {
      Some(Response::Batch(responses))
    } else {
      Some(Response::Single(
        responses.pop().expect("one response to a single call"),
      ))
    }
  }

  pub(super) fn reject(&mut self, error: &RequestError) {
    self.observations.clear();
    for call in &mut self.calls {
      if !call.is_complete() {
        call.complete(Err(RpcError::from(error)));
      }
    }
  }
}

fn prepare_method(space: Space, name: &str, raw: Option<&RawValue>) -> Result<Operation, RpcError> {
  if let Some(request) = deserialize_request::<DevRequest>(name, raw)? {
    return Ok(request.into_operation());
  }
  match space {
    Space::Native => Err(RpcError::method_not_found()),
    Space::Ethereum => {
      let request =
        deserialize_request::<EspaceRequest>(name, raw)?.ok_or_else(RpcError::method_not_found)?;
      Ok(request.into_operation())
    }
  }
}
