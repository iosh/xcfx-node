use std::{
  collections::{HashMap, HashSet, VecDeque},
  sync::{Arc, Mutex},
};

use cfx_types::Space;
use serde::Serialize;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::node::{Cancellation, MessageId, NodeError, NodeHandle, RequestError};

use super::service::Command;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ConnectionId(pub(super) u64);

/// One owner of a transport-independent connection and its ordered output.
pub(crate) struct Connection {
  handle: ConnectionHandle,
  output: Arc<OutboundQueue>,
}

#[derive(Clone)]
pub(crate) struct ConnectionHandle {
  pub(super) id: ConnectionId,
  pub(super) space: Space,
  node: NodeHandle,
  commands: mpsc::Sender<Command>,
  disconnects: mpsc::UnboundedSender<ConnectionId>,
  state: Arc<Mutex<ConnectionState>>,
  closed: Cancellation,
}

struct ConnectionState {
  next_request: u64,
  requests: HashMap<u64, Cancellation>,
}

pub(crate) struct RequestHandle {
  cancellation: Cancellation,
  calls: HashMap<usize, Cancellation>,
}

impl RequestHandle {
  pub(crate) fn cancel(&self) {
    self.cancellation.cancel();
  }

  pub(crate) fn cancel_call(&self, index: usize) {
    if let Some(call) = self.calls.get(&index) {
      call.cancel();
    }
  }
}

/// Owns registration and cancellation through response completion, including
/// the time an admitted command spends in the IO queue.
pub(super) struct RequestGuard {
  connection: ConnectionHandle,
  id: u64,
  cancellation: Cancellation,
}

impl RequestGuard {
  pub(super) fn connection(&self) -> &ConnectionHandle {
    &self.connection
  }

  pub(super) fn cancellation(&self) -> &Cancellation {
    &self.cancellation
  }
}

impl Drop for RequestGuard {
  fn drop(&mut self) {
    self.cancellation.cancel();
    self.connection.complete(self.id);
  }
}

// Cancelling send before it hands back a handle still cancels an admitted message.
struct PendingSend(Option<Cancellation>);

impl Drop for PendingSend {
  fn drop(&mut self) {
    if let Some(cancellation) = &self.0 {
      cancellation.cancel();
    }
  }
}

impl Connection {
  pub(super) async fn new(
    id: ConnectionId,
    space: Space,
    notifications: bool,
    node: NodeHandle,
    commands: mpsc::Sender<Command>,
    disconnects: mpsc::UnboundedSender<ConnectionId>,
  ) -> Result<Self, RequestError> {
    let output = Arc::new(OutboundQueue::default());
    let handle = ConnectionHandle {
      id,
      space,
      node,
      commands,
      disconnects,
      state: Arc::new(Mutex::new(ConnectionState {
        next_request: 1,
        requests: HashMap::new(),
      })),
      closed: Cancellation::default(),
    };
    let permit = handle.reserve().await?;
    handle.node.admit(
      permit,
      Command::Connect {
        handle: handle.clone(),
        output: OutboundSender {
          queue: Arc::clone(&output),
        },
        notifications,
      },
      None,
    )?;
    Ok(Self { handle, output })
  }

  pub(crate) fn handle(&self) -> ConnectionHandle {
    self.handle.clone()
  }

  /// A transport must await its own output readiness before receiving, so
  /// subscription messages remain reclaimable while the transport is blocked.
  pub(crate) async fn recv(&mut self) -> Option<String> {
    self.output.recv().await
  }

  pub(crate) fn close(&self) {
    self.handle.close();
  }
}

impl Drop for Connection {
  fn drop(&mut self) {
    self.close();
    self.output.disconnect();
  }
}

impl ConnectionHandle {
  /// Waits for ingress queue capacity and protocol preparation, not execution or a response.
  /// Dropping this future cancels the message. Dropping the returned handle does
  /// not cancel it; notifications also have an explicitly cancellable lifetime.
  pub(crate) async fn send(&self, payload: Vec<u8>) -> Result<RequestHandle, RequestError> {
    let admission = self.reserve().await?;
    let cancellation = Cancellation::default();
    let id = {
      let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
      if self.is_closed() {
        return Err(NodeError::Closed.into());
      }
      let id = state.next_request;
      state.next_request = id
        .checked_add(1)
        .expect("connection request ID space exhausted");
      state.requests.insert(id, cancellation.clone());
      id
    };
    // The registration lock is released before publishing or dropping its guard.
    // A racing disconnect cancels this signal even if it reaches IO first.
    let request = RequestGuard {
      connection: self.clone(),
      id,
      cancellation: cancellation.clone(),
    };
    let mut pending = PendingSend(Some(cancellation.clone()));
    let (accepted, acceptance) = oneshot::channel();
    self.node.admit(
      admission,
      Command::Request {
        request,
        payload,
        accepted,
      },
      Some(&cancellation),
    )?;
    let calls = acceptance.await.map_err(|_| self.service_error())?;
    pending.0 = None;
    Ok(RequestHandle {
      cancellation,
      calls,
    })
  }

  async fn reserve(&self) -> Result<mpsc::Permit<'_, Command>, RequestError> {
    tokio::select! {
      biased;
      _ = self.node.stopped() => Err(self.stopped_error()),
      _ = self.closed.cancelled() => Err(NodeError::Closed.into()),
      permit = self.commands.reserve() => permit.map_err(|_| self.service_error()),
    }
  }

  fn stopped_error(&self) -> RequestError {
    self
      .node
      .check_ready()
      .expect_err("a stopped node is unavailable")
      .into()
  }

  fn service_error(&self) -> RequestError {
    // The receiver may disappear before IO supervision records its failure.
    self
      .node
      .check_ready()
      .err()
      .unwrap_or(NodeError::IoServiceStopped(None))
      .into()
  }

  pub(crate) fn close(&self) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if !self.is_closed() {
      self.closed.cancel();
      for cancellation in state.requests.values() {
        cancellation.cancel();
      }
      state.requests.clear();
      // At most one control message per connection; it never waits on ingress.
      let _ = self.disconnects.send(self.id);
    }
  }

  fn complete(&self, request: u64) {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .requests
      .remove(&request);
  }

  fn is_closed(&self) -> bool {
    self.closed.is_cancelled()
  }
}

enum Outbound {
  Response {
    text: String,
  },
  Notification {
    text: String,
    subscription: SubscriptionId,
  },
}

impl Outbound {
  fn subscription(&self) -> Option<SubscriptionId> {
    match self {
      Self::Response { .. } => None,
      Self::Notification { subscription, .. } => Some(*subscription),
    }
  }

  fn into_text(self) -> String {
    match self {
      Self::Response { text } | Self::Notification { text, .. } => text,
    }
  }
}

/// One ordered queue whose subscription entries can be reclaimed without a reader.
#[derive(Default)]
struct OutboundQueue {
  state: Mutex<OutboundState>,
  changed: Notify,
}

// The unique producer closes output even if its queued Connect is never processed.
pub(super) struct OutboundSender {
  queue: Arc<OutboundQueue>,
}

impl Drop for OutboundSender {
  fn drop(&mut self) {
    self.queue.close();
  }
}

#[derive(Default)]
struct OutboundState {
  messages: VecDeque<Outbound>,
  notifications: HashMap<SubscriptionId, usize>,
  closed: bool,
}

impl OutboundQueue {
  async fn recv(&self) -> Option<String> {
    loop {
      let changed = self.changed.notified();
      tokio::pin!(changed);
      changed.as_mut().enable();
      {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(message) = state.messages.pop_front() {
          if let Some(id) = message.subscription() {
            let count = state
              .notifications
              .get_mut(&id)
              .expect("queued notification has a count");
            *count -= 1;
            if *count == 0 {
              state.notifications.remove(&id);
            }
          }
          return Some(message.into_text());
        }
        if state.closed {
          return None;
        }
      }
      changed.await;
    }
  }

  fn push(&self, message: Outbound) -> bool {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    if state.closed {
      return false;
    }
    if let Some(id) = message.subscription() {
      *state.notifications.entry(id).or_default() += 1;
    }
    state.messages.push_back(message);
    drop(state);
    self.changed.notify_one();
    true
  }

  fn notification_count(&self, id: SubscriptionId) -> usize {
    self
      .state
      .lock()
      .unwrap_or_else(|e| e.into_inner())
      .notifications
      .get(&id)
      .copied()
      .unwrap_or(0)
  }

  fn remove_subscription(&self, id: SubscriptionId) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state
      .messages
      .retain(|message| message.subscription() != Some(id));
    state.notifications.remove(&id);
  }

  fn disconnect(&self) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state.closed = true;
    state.messages.clear();
    state.notifications.clear();
    drop(state);
    self.changed.notify_waiters();
  }

  fn close(&self) {
    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
    state.closed = true;
    // Responses remain readable after shutdown; subscriptions end with the connection.
    state
      .messages
      .retain(|message| message.subscription().is_none());
    state.notifications.clear();
    drop(state);
    self.changed.notify_waiters();
  }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct SubscriptionId(u64);

impl Serialize for SubscriptionId {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("0x{:x}", self.0))
  }
}

struct Notification {
  cause: Option<MessageId>,
  text: String,
}

struct Subscription {
  created_by: MessageId,
  limit: usize,
  pending: VecDeque<Notification>,
}

struct Output {
  handle: ConnectionHandle,
  sender: OutboundSender,
  notifications: Option<NotificationState>,
}

#[derive(Default)]
struct NotificationState {
  pending: HashSet<MessageId>,
  subscriptions: HashMap<SubscriptionId, Subscription>,
}

/// Access to one connection operation at an observed execution boundary.
pub(super) struct ConnectionCall<'a> {
  connections: &'a mut Connections,
  connection: ConnectionId,
  message: MessageId,
}

impl ConnectionCall<'_> {
  pub(super) fn subscribe(self, limit: usize) -> Option<SubscriptionId> {
    self
      .connections
      .subscribe(self.connection, self.message, limit)
  }

  pub(super) fn unsubscribe(self, id: SubscriptionId) -> bool {
    self.connections.unsubscribe(self.connection, id)
  }
}

/// Owned exclusively by the IO service. Socket readiness never blocks this owner.
#[derive(Default)]
pub(super) struct Connections {
  outputs: HashMap<ConnectionId, Output>,
  last_subscription: u64,
}

impl Connections {
  pub(super) fn insert(
    &mut self,
    handle: ConnectionHandle,
    sender: OutboundSender,
    notifications: bool,
  ) {
    if !handle.is_closed() {
      self.outputs.insert(
        handle.id,
        Output {
          handle,
          sender,
          notifications: notifications.then(NotificationState::default),
        },
      );
    }
  }

  pub(super) fn remove(&mut self, id: ConnectionId) {
    if let Some(output) = self.outputs.remove(&id) {
      output.handle.close();
    }
  }

  pub(super) fn begin(&mut self, connection: ConnectionId, message: MessageId) {
    if let Some(output) = self.outputs.get_mut(&connection)
      && let Some(notifications) = output.notifications.as_mut()
    {
      notifications.pending.insert(message);
    }
  }

  pub(super) fn observation(
    &mut self,
    connection: ConnectionId,
    message: MessageId,
  ) -> ConnectionCall<'_> {
    ConnectionCall {
      connections: self,
      connection,
      message,
    }
  }

  /// Enqueues the response before releasing its notification gate.
  pub(super) fn complete(
    &mut self,
    request: RequestGuard,
    message: Option<MessageId>,
    response: Option<String>,
  ) {
    let connection = request.connection.id;
    if let Some(text) = response
      && let Some(output) = self.outputs.get(&connection)
      && !output.sender.queue.push(Outbound::Response { text })
    {
      self.remove(connection);
    }
    if let Some(message) = message
      && let Some(output) = self.outputs.get_mut(&connection)
      && let Some(notifications) = output.notifications.as_mut()
    {
      notifications.pending.remove(&message);
      for (&id, subscription) in &mut notifications.subscriptions {
        Self::flush(
          &output.sender.queue,
          &notifications.pending,
          id,
          subscription,
        );
      }
    }
    // RequestGuard is released after the response and all newly eligible notifications.
  }

  fn subscribe(
    &mut self,
    connection: ConnectionId,
    message: MessageId,
    limit: usize,
  ) -> Option<SubscriptionId> {
    let output = self.outputs.get_mut(&connection)?;
    if output.handle.is_closed() {
      return None;
    }
    let notifications = output.notifications.as_mut()?;
    self.last_subscription = self
      .last_subscription
      .checked_add(1)
      .expect("subscription ID space exhausted");
    let id = SubscriptionId(self.last_subscription);
    notifications.subscriptions.insert(
      id,
      Subscription {
        created_by: message,
        limit,
        pending: VecDeque::new(),
      },
    );
    Some(id)
  }

  fn unsubscribe(&mut self, connection: ConnectionId, id: SubscriptionId) -> bool {
    let Some(output) = self.outputs.get_mut(&connection) else {
      return false;
    };
    let Some(notifications) = output.notifications.as_mut() else {
      return false;
    };
    if notifications.subscriptions.remove(&id).is_none() {
      return false;
    }
    output.sender.queue.remove_subscription(id);
    true
  }

  /// Internal waiters observe facts before this notification-only gate. Later
  /// notifications cannot pass a blocked notification of the same subscription.
  pub(super) fn notify(
    &mut self,
    connection: ConnectionId,
    id: SubscriptionId,
    cause: Option<MessageId>,
    text: String,
  ) {
    let Some(output) = self.outputs.get_mut(&connection) else {
      return;
    };
    let Some(notifications) = output.notifications.as_mut() else {
      return;
    };
    let Some(subscription) = notifications.subscriptions.get_mut(&id) else {
      return;
    };
    // The only concurrent consumer can reduce this count but cannot increase it.
    let queued = output.sender.queue.notification_count(id);
    if queued >= subscription.limit || subscription.pending.len() >= subscription.limit - queued {
      notifications.subscriptions.remove(&id);
      output.sender.queue.remove_subscription(id);
      return;
    }
    subscription.pending.push_back(Notification { cause, text });
    Self::flush(
      &output.sender.queue,
      &notifications.pending,
      id,
      subscription,
    );
  }

  fn flush(
    queue: &OutboundQueue,
    pending: &HashSet<MessageId>,
    id: SubscriptionId,
    subscription: &mut Subscription,
  ) {
    // Facts from another request or connection must also wait for installation's reply.
    if pending.contains(&subscription.created_by) {
      return;
    }
    while subscription
      .pending
      .front()
      .is_some_and(|item| item.cause.is_none_or(|id| !pending.contains(&id)))
    {
      let item = subscription
        .pending
        .pop_front()
        .expect("the subscription queue is nonempty");
      if !queue.push(Outbound::Notification {
        text: item.text,
        subscription: id,
      }) {
        break;
      }
    }
  }

  pub(super) fn close(&mut self) {
    for (_, output) in self.outputs.drain() {
      output.handle.close();
    }
  }
}

impl Drop for Connections {
  fn drop(&mut self) {
    self.close();
  }
}
