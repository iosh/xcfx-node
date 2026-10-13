use std::{
  collections::HashMap,
  future::Future,
  panic::{AssertUnwindSafe, catch_unwind},
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};

use cfx_types::Space;
use tokio::{
  sync::{mpsc, oneshot, watch},
  task::{Id as TaskId, JoinError, JoinSet},
};

use crate::{
  config::NodeConfig,
  node::{
    BatchResult, CallId, Cancellation, MessageId, MiningCause, MiningError, NodeError, NodeEvent,
    NodeEvents, NodeHandle, NodeService, NodeStartError, RequestError,
  },
};

use super::{
  connection::{
    Connection, ConnectionHandle, ConnectionId, Connections, OutboundSender, RequestGuard,
  },
  dispatch::MessageDispatch,
  operation::{CompletionContext, PendingResponse, ResponseConversion, RpcResult},
  request::{Id, Request},
  response::{Response, RpcError, RpcResponse},
};

const INGRESS_CAPACITY: usize = 64;

type ExecutionResult = BatchResult<ResponseConversion>;

pub(crate) struct MiningDiagnostic {
  pub(crate) cause: MiningCause,
  pub(crate) error: Arc<MiningError>,
}

/// A request-local completion panic. Payloads are never part of RPC errors.
pub(crate) struct CompletionDiagnostic {
  pub(crate) call: CallId,
  pub(crate) source: Option<Arc<JoinError>>,
}

/// Owns the execution service; connections cannot keep the instance alive.
pub(crate) struct RpcService {
  node: NodeService,
  commands: mpsc::Sender<Command>,
  disconnects: mpsc::UnboundedSender<ConnectionId>,
  next_connection: AtomicU64,
  mining_diagnostic: watch::Receiver<Option<Arc<MiningDiagnostic>>>,
  completion_diagnostic: watch::Receiver<Option<Arc<CompletionDiagnostic>>>,
}

impl RpcService {
  pub(crate) async fn start(config: NodeConfig) -> Result<Self, NodeStartError> {
    let (commands, receiver) = mpsc::channel(INGRESS_CAPACITY);
    let (disconnects, disconnect_receiver) = mpsc::unbounded_channel();
    let (mining_diagnostic, mining_latest) = watch::channel(None);
    let (completion_diagnostic, completion_latest) = watch::channel(None);
    let node = NodeService::start(config, move |node, facts| {
      run(
        node,
        facts,
        receiver,
        disconnect_receiver,
        mining_diagnostic,
        completion_diagnostic,
      )
    })
    .await?;
    Ok(Self {
      node,
      commands,
      disconnects,
      next_connection: AtomicU64::new(1),
      mining_diagnostic: mining_latest,
      completion_diagnostic: completion_latest,
    })
  }

  /// HTTP uses notifications=false; WS and in-process connections use true.
  pub(crate) async fn connect(
    &self,
    space: Space,
    notifications: bool,
  ) -> Result<Connection, RequestError> {
    self.node.handle().check_ready()?;
    let id = self
      .next_connection
      .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
      .expect("connection ID space exhausted");
    Connection::new(
      ConnectionId(id),
      space,
      notifications,
      self.node.handle(),
      self.commands.clone(),
      self.disconnects.clone(),
    )
    .await
  }

  /// A latest-value diagnostic view, not a lossless event or logging stream.
  pub(crate) fn mining_diagnostic(&self) -> watch::Receiver<Option<Arc<MiningDiagnostic>>> {
    self.mining_diagnostic.clone()
  }

  pub(crate) fn completion_diagnostic(&self) -> watch::Receiver<Option<Arc<CompletionDiagnostic>>> {
    self.completion_diagnostic.clone()
  }

  pub(crate) fn close(&self) -> impl Future<Output = Result<(), NodeError>> + '_ {
    self.node.close()
  }
}

pub(super) enum Command {
  Connect {
    handle: ConnectionHandle,
    output: OutboundSender,
    notifications: bool,
  },
  Request {
    request: RequestGuard,
    payload: Vec<u8>,
    accepted: oneshot::Sender<HashMap<usize, Cancellation>>,
  },
}

/// Identity, completed slots, and cancellation outlive both kinds of async task.
struct InFlightRequest {
  request: RequestGuard,
  calls: HashMap<usize, Cancellation>,
  dispatch: MessageDispatch,
  completions: usize,
}

enum CompletionTask {
  Wait(CallId),
  Observation(CallId),
}

impl CompletionTask {
  fn call(&self) -> CallId {
    match self {
      Self::Wait(call) | Self::Observation(call) => *call,
    }
  }
}

struct Dispatcher {
  node: NodeHandle,
  connections: Connections,
  pending: HashMap<MessageId, InFlightRequest>,
  executions: JoinSet<ExecutionResult>,
  execution_messages: HashMap<TaskId, MessageId>,
  completions: JoinSet<RpcResult>,
  completion_calls: HashMap<TaskId, CompletionTask>,
  changes: watch::Sender<()>,
  completion_diagnostic: watch::Sender<Option<Arc<CompletionDiagnostic>>>,
}

async fn run(
  node: NodeHandle,
  mut facts: NodeEvents,
  mut commands: mpsc::Receiver<Command>,
  mut disconnects: mpsc::UnboundedReceiver<ConnectionId>,
  mining_diagnostic: watch::Sender<Option<Arc<MiningDiagnostic>>>,
  completion_diagnostic: watch::Sender<Option<Arc<CompletionDiagnostic>>>,
) {
  let (changes, _) = watch::channel(());
  let mut dispatcher = Dispatcher {
    node: node.clone(),
    connections: Connections::default(),
    pending: HashMap::new(),
    executions: JoinSet::new(),
    execution_messages: HashMap::new(),
    completions: JoinSet::new(),
    completion_calls: HashMap::new(),
    changes,
    completion_diagnostic,
  };
  let mut stopping = false;
  let mut facts_open = true;
  let mut commands_open = true;
  let mut disconnects_open = true;
  while facts_open
    || commands_open
    || !dispatcher.executions.is_empty()
    || !dispatcher.completions.is_empty()
  {
    tokio::select! {
      _ = node.stopped(), if !stopping => {
        stopping = true;
        dispatcher.drain_ingress(&mut commands);
        commands_open = false;
      }
      event = facts.recv(), if facts_open => {
        match event {
          Some(event) => dispatcher.consume(event, &mining_diagnostic),
          None => {
            facts_open = false;
            if node.check_ready().is_ok() {
              node.fail_io_service(None);
            }
            stopping = true;
            dispatcher.drain_ingress(&mut commands);
            commands_open = false;
            for active in dispatcher.pending.values_mut() {
              active.dispatch.close_observations();
            }
          }
        }
      }
      command = commands.recv(), if commands_open => {
        match command {
          Some(command) => dispatcher.accept(command, stopping),
          None => {
            commands_open = false;
            if node.check_ready().is_ok() {
              node.fail_io_service(None);
            }
          }
        }
      }
      connection = disconnects.recv(), if disconnects_open => {
        match connection {
          Some(id) => dispatcher.connections.remove(id),
          None => disconnects_open = false,
        }
      }
      result = dispatcher.executions.join_next_with_id(), if !dispatcher.executions.is_empty() => {
        dispatcher.executed(result.expect("an active execution task completes"));
      }
      result = dispatcher.completions.join_next_with_id(), if !dispatcher.completions.is_empty() => {
        dispatcher.completed(result.expect("an active response task completes"));
      }
    }
  }
  assert!(
    dispatcher.pending.is_empty(),
    "all accepted requests have completed"
  );
  // Shutdown never waits for transports to consume their retained responses.
  dispatcher.connections.close();
}

impl Dispatcher {
  fn drain_ingress(&mut self, commands: &mut mpsc::Receiver<Command>) {
    commands.close();
    // Lifecycle::admit cannot publish after stop. A suspended producer may still
    // own a reserved channel slot, so recv().await would delay shutdown forever.
    while let Ok(command) = commands.try_recv() {
      self.accept(command, true);
    }
  }

  fn consume(
    &mut self,
    event: NodeEvent,
    diagnostic: &watch::Sender<Option<Arc<MiningDiagnostic>>>,
  ) {
    match event {
      NodeEvent::Committed { cause: _, outcome } => {
        self.changes.send_replace(());
        // Notification projection consumes the same ordered facts. Its output
        // budget must never retain or block this internal wakeup stream.
        drop(outcome);
      }
      NodeEvent::MiningFailed { cause, error } => {
        diagnostic.send_replace(Some(Arc::new(MiningDiagnostic { cause, error })));
      }
      NodeEvent::Observation { call } => {
        let applied = catch_unwind(AssertUnwindSafe(|| {
          let active = self
            .pending
            .get_mut(&call.message)
            .expect("an observation's request outlives its marker");
          let connection = self
            .connections
            .observation(active.request.connection().id, call.message);
          active
            .dispatch
            .observe(call.index, connection, || self.node.fail_io_service(None));
        }));
        if applied.is_err() {
          self.node.fail_io_service(None);
        }
      }
    }
  }

  fn accept(&mut self, command: Command, stopping: bool) {
    match command {
      Command::Connect {
        handle,
        output,
        notifications,
      } => {
        if stopping || self.node.check_ready().is_err() {
          handle.close();
        } else {
          self.connections.insert(handle, output, notifications);
        }
      }
      Command::Request {
        request,
        payload,
        accepted,
      } => {
        self.request(request, payload, accepted);
      }
    }
  }

  fn request(
    &mut self,
    request: RequestGuard,
    payload: Vec<u8>,
    accepted: oneshot::Sender<HashMap<usize, Cancellation>>,
  ) {
    // Before parsing succeeds there is no established response identity.
    let parsed = catch_unwind(AssertUnwindSafe(|| Request::parse(&payload))).unwrap_or_else(|_| {
      self.node.fail_io_service(None);
      Err(RpcResponse::new(Id::Null, Err(RpcError::internal())))
    });
    let parsed = match parsed {
      Ok(parsed) => parsed,
      Err(error) => {
        let _ = accepted.send(HashMap::new());
        self.connections.complete(
          request,
          None,
          serialize_response(Some(Response::Single(error))),
        );
        return;
      }
    };
    let mut dispatch = MessageDispatch::new(parsed);
    let rejection = if request.cancellation().is_cancelled() {
      Some(RequestError::Cancelled)
    } else {
      self.node.check_ready().err().map(RequestError::from)
    };
    if let Some(error) = rejection {
      dispatch.reject(&error);
      let _ = accepted.send(HashMap::new());
      self
        .connections
        .complete(request, None, serialize_response(dispatch.into_response()));
      return;
    }
    let work = catch_unwind(AssertUnwindSafe(|| {
      dispatch.prepare(request.connection().space)
    }))
    .unwrap_or_else(|_| {
      self.node.fail_io_service(None);
      // No work was admitted. Completed protocol results and every ID survive.
      Vec::new()
    });
    if work.is_empty() {
      let _ = accepted.send(HashMap::new());
      self
        .connections
        .complete(request, None, serialize_response(dispatch.into_response()));
      return;
    }
    let execution = self.node.request(work, request.cancellation().clone());
    let message = execution.id();
    let calls: HashMap<_, _> = execution.call_cancellations().collect();
    self.connections.begin(request.connection().id, message);
    let task = self.executions.spawn(execution.execute());
    self.execution_messages.insert(task.id(), message);
    let accepted_calls = calls.clone();
    self.pending.insert(
      message,
      InFlightRequest {
        request,
        calls,
        dispatch,
        completions: 0,
      },
    );
    let _ = accepted.send(accepted_calls);
  }

  fn executed(&mut self, result: Result<(TaskId, ExecutionResult), JoinError>) {
    let (id, outcome) = match result {
      Ok(result) => result,
      Err(error) => {
        let id = error.id();
        let source = Some(Arc::new(error));
        self.node.fail_io_service(source.clone());
        (id, Err(NodeError::IoServiceStopped(source).into()))
      }
    };
    let message = self
      .execution_messages
      .remove(&id)
      .expect("an execution task retains its message identity");
    let active = self
      .pending
      .get_mut(&message)
      .expect("an executing request is registered");
    let converted = catch_unwind(AssertUnwindSafe(|| match outcome {
      Ok(results) => active.dispatch.complete(results, |index| {
        self
          .completion_diagnostic
          .send_replace(Some(Arc::new(CompletionDiagnostic {
            call: CallId { message, index },
            source: None,
          })));
      }),
      Err(error) => {
        active.dispatch.reject(&error);
        Vec::new()
      }
    }));
    let waiting = converted.unwrap_or_else(|_| {
      self.node.fail_io_service(None);
      active
        .dispatch
        .reject(&NodeError::IoServiceStopped(None).into());
      Vec::new()
    });
    active.completions = waiting.len();
    if waiting.is_empty() {
      self.finish(message);
      return;
    }
    for (index, response) in waiting {
      self.start_completion(CallId { message, index }, response);
    }
  }

  fn start_completion(&mut self, call: CallId, response: PendingResponse) {
    let (task, owner) = match response {
      PendingResponse::Wait(complete) => {
        let active = self
          .pending
          .get(&call.message)
          .expect("a waiting request is registered");
        let cancellation = active.request.cancellation().clone();
        let call_cancellation = active
          .calls
          .get(&call.index)
          .expect("a waiting call has cancellation")
          .clone();
        let node = self.node.clone();
        let context = CompletionContext::new(node.clone(), self.changes.subscribe());
        let task = self.completions.spawn(async move {
          tokio::select! {
            biased;
            _ = node.stopped() => {
              let error = node.check_ready().expect_err("a stopped node is unavailable");
              Err(RpcError::from(&RequestError::Node(error)))
            }
            _ = cancellation.cancelled() => Err(RpcError::from(&RequestError::Cancelled)),
            _ = call_cancellation.cancelled() => Err(RpcError::from(&RequestError::Cancelled)),
            result = async move { complete(context).await } => result,
          }
        });
        (task, CompletionTask::Wait(call))
      }
      PendingResponse::Observation(response) => {
        // Worker success means the operation crossed its start boundary. Even
        // stop cannot skip its marker; draining facts supplies this result.
        let task = self.completions.spawn(async move {
          response
            .await
            .expect("a started observation delivers its outcome")
        });
        (task, CompletionTask::Observation(call))
      }
    };
    self.completion_calls.insert(task.id(), owner);
  }

  fn completed(&mut self, result: Result<(TaskId, RpcResult), JoinError>) {
    let id = match &result {
      Ok((id, _)) => *id,
      Err(error) => error.id(),
    };
    let owner = self
      .completion_calls
      .remove(&id)
      .expect("a completion task retains its call identity");
    let call = owner.call();
    let response = match result {
      Ok((_, response)) => response,
      Err(error) => {
        let isolated = matches!(&owner, CompletionTask::Wait(_)) && error.is_panic();
        let source = Some(Arc::new(error));
        if isolated {
          self
            .completion_diagnostic
            .send_replace(Some(Arc::new(CompletionDiagnostic { call, source })));
        } else {
          // Observation delivery and unexpected task cancellation belong to
          // shared protocol supervision, not the request-local panic boundary.
          self.node.fail_io_service(source);
        }
        Err(RpcError::internal())
      }
    };
    let active = self
      .pending
      .get_mut(&call.message)
      .expect("a completing request is registered");
    active.dispatch.complete_waiting(call.index, response);
    active.completions = active
      .completions
      .checked_sub(1)
      .expect("a completion was outstanding");
    if active.completions == 0 {
      self.finish(call.message);
    }
  }

  fn finish(&mut self, message: MessageId) {
    let active = self
      .pending
      .remove(&message)
      .expect("a finished request owns its response");
    assert_eq!(active.completions, 0, "all response tasks have completed");
    self.connections.complete(
      active.request,
      Some(message),
      serialize_response(active.dispatch.into_response()),
    );
  }
}

fn serialize_response(response: Option<Response>) -> Option<String> {
  response.map(|response| {
    serde_json::to_string(&response).expect("RPC responses contain serializable JSON values")
  })
}
