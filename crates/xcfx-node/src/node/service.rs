//! Asynchronous admission and the owner of an instance's execution and IO threads.

use std::{
  future::Future,
  mem,
  panic::{AssertUnwindSafe, catch_unwind},
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
  thread,
};

use tokio::{
  runtime::{Builder, Runtime},
  sync::{mpsc, oneshot, watch},
  task::{JoinError, JoinHandle},
};

use crate::config::{MiningMode, NodeConfig};

use super::{
  NodeError, NodeEvent, NodeStartError,
  lifecycle::{Lifecycle, NodeStatus},
  mining::{interval_deadline, run_timer},
  request::{BatchResult, Call, Cancellation, Message, MessageId, NodeTask, RequestError, Work},
  worker,
};

// Saturation delays admission without limiting the number of accepted operations.
const COMMAND_QUEUE_CAPACITY: usize = 64;

/// The instance-internal fact stream. Its consumer must drain it promptly;
/// bounded per-subscription buffering belongs downstream.
pub(crate) type NodeEvents = mpsc::UnboundedReceiver<NodeEvent>;

/// The unique instance owner. Dropping it starts the same cleanup as `close`.
/// Cloned handles and background tasks do not keep this owner alive.
pub(crate) struct NodeService {
  handle: NodeHandle,
}

impl NodeService {
  /// Starts an instance without requiring a Tokio runtime on the calling thread.
  /// Startup failure awaits resource cleanup; dropping this future also initiates
  /// cleanup, which continues independently of the caller.
  pub(crate) async fn start<F, Fut>(
    config: NodeConfig,
    io_service: F,
  ) -> Result<Self, NodeStartError>
  where
    F: FnOnce(NodeHandle, NodeEvents) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
  {
    config.validate()?;
    if let MiningMode::Interval(period) = config.mining {
      // Initialization must not consume the first scheduling interval.
      interval_deadline(period)?;
    }
    let lifecycle = Arc::new(Lifecycle::new());
    let (requests, receiver) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
    let (events, event_receiver) = mpsc::unbounded_channel();
    let (ready, started) = oneshot::channel();
    let service = Self {
      handle: NodeHandle {
        requests: requests.clone(),
        lifecycle: Arc::clone(&lifecycle),
        next_message: Arc::new(AtomicU64::new(1)),
      },
    };

    let io_lifecycle = Arc::clone(&lifecycle);
    let io_handle = service.handle();
    if let Err(source) = thread::Builder::new()
      .name("xcfx-io".into())
      .spawn(move || {
        run_io(
          config,
          requests,
          receiver,
          events,
          ready,
          io_lifecycle,
          move || io_service(io_handle, event_receiver),
        )
      })
    {
      lifecycle.stop();
      lifecycle.finish(Ok(()));
      return Err(NodeStartError::Thread {
        thread: "IO",
        source,
      });
    }

    let result = started
      .await
      .unwrap_or_else(|_| Err(NodeStartError::Service(lifecycle.unavailable())))
      // The service can fail while its successful receipt waits to be polled.
      .and_then(|()| lifecycle.check_ready().map_err(NodeStartError::from));
    match result {
      Ok(()) => Ok(service),
      Err(error) => {
        // Finish cleanup before reporting failure, retaining the final service
        // diagnostic alongside a stage-specific startup error.
        match service.close().await {
          Ok(()) => Err(error),
          Err(service_error) => Err(error.with_service_failure(service_error)),
        }
      }
    }
  }

  pub(crate) fn handle(&self) -> NodeHandle {
    self.handle.clone()
  }

  pub(crate) fn status(&self) -> NodeStatus {
    self.handle.status()
  }

  /// Synchronously closes admission. The current indivisible operation may finish.
  pub(crate) fn stop(&self) {
    self.handle.lifecycle.stop();
  }

  /// Waits for the execution thread and IO runtime to be reclaimed.
  /// Calling this closes admission even if the returned future is never polled.
  /// Cancelling the wait does not cancel cleanup; all waiters receive the same result.
  pub(crate) fn close(&self) -> impl Future<Output = Result<(), NodeError>> + '_ {
    self.stop();
    self.handle.lifecycle.closed()
  }
}

impl Drop for NodeService {
  fn drop(&mut self) {
    self.stop();
  }
}

/// A cloneable command producer with no access to the Runtime or shutdown ownership.
#[derive(Clone)]
pub(crate) struct NodeHandle {
  requests: mpsc::Sender<Work>,
  lifecycle: Arc<Lifecycle>,
  next_message: Arc<AtomicU64>,
}

impl NodeHandle {
  pub(crate) fn status(&self) -> NodeStatus {
    self.lifecycle.status()
  }

  /// Creates a grouped request and its cancellation signals before admission.
  pub(crate) fn request<T: Send + 'static>(
    &self,
    tasks: Vec<(usize, NodeTask<T>)>,
    cancellation: Cancellation,
  ) -> NodeRequest<T> {
    let sequence = self
      .next_message
      .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_add(1)
      })
      .expect("node message ID space exhausted");
    NodeRequest {
      handle: self.clone(),
      id: MessageId(sequence),
      calls: tasks
        .into_iter()
        .map(|(index, task)| Call {
          index,
          task,
          cancellation: Cancellation::default(),
        })
        .collect(),
      cancellation,
      cancel_on_drop: true,
    }
  }

  pub(crate) async fn stopped(&self) {
    self.lifecycle.stopped().await;
  }

  pub(crate) fn check_ready(&self) -> Result<(), NodeError> {
    self.lifecycle.check_ready()
  }

  /// Publishes already reserved work under the same boundary as instance stop.
  pub(crate) fn admit<T>(
    &self,
    permit: mpsc::Permit<'_, T>,
    value: T,
    cancellation: Option<&Cancellation>,
  ) -> Result<(), RequestError> {
    self.lifecycle.admit(permit, value, cancellation)
  }

  pub(crate) fn fail_io_service(&self, source: Option<Arc<JoinError>>) {
    self.lifecycle.fail(NodeError::IoServiceStopped(source));
  }
}

/// Owns one batch's admission and execution. Dropping an unfinished request
/// signals cancellation; a returned result disarms that guard.
pub(crate) struct NodeRequest<T> {
  handle: NodeHandle,
  id: MessageId,
  calls: Vec<Call<T>>,
  cancellation: Cancellation,
  cancel_on_drop: bool,
}

impl<T: Send + 'static> NodeRequest<T> {
  pub(crate) fn id(&self) -> MessageId {
    self.id
  }

  pub(crate) fn call_cancellations(&self) -> impl Iterator<Item = (usize, Cancellation)> + '_ {
    self
      .calls
      .iter()
      .map(|call| (call.index, call.cancellation.clone()))
  }

  /// Waits for queue capacity, all task results, and the message's automatic mining.
  /// After admission, cancellation is resolved by the worker so partial mining
  /// results survive in the batch reply. Notifications also await completion.
  pub(crate) async fn execute(mut self) -> BatchResult<T> {
    let result = self.admit_and_wait().await;
    self.cancel_on_drop = false;
    result
  }

  async fn admit_and_wait(&mut self) -> BatchResult<T> {
    let permit = tokio::select! {
      biased;
      _ = self.handle.lifecycle.stopped() => return Err(self.handle.lifecycle.unavailable().into()),
      _ = self.cancellation.cancelled() => return Err(RequestError::Cancelled),
      permit = self.handle.requests.reserve() => {
        permit.map_err(|_| self.handle.lifecycle.unavailable())?
      }
    };
    let (reply, response) = oneshot::channel();
    let message = Message {
      id: self.id,
      calls: mem::take(&mut self.calls),
      cancellation: self.cancellation.clone(),
      reply,
    };
    self.handle.admit(
      permit,
      Work::Message(Box::new(message)),
      Some(&self.cancellation),
    )?;
    response
      .await
      .unwrap_or_else(|_| Err(self.handle.lifecycle.unavailable().into()))
  }
}

impl<T> Drop for NodeRequest<T> {
  fn drop(&mut self) {
    if self.cancel_on_drop {
      self.cancellation.cancel();
    }
  }
}

struct IoProgress {
  ready: Option<oneshot::Sender<Result<(), NodeStartError>>>,
  started: Option<oneshot::Receiver<Result<(), NodeStartError>>>,
  finished: Option<oneshot::Receiver<()>>,
  timer: Option<JoinHandle<()>>,
  service: Option<JoinHandle<()>>,
}

impl IoProgress {
  fn report_start(&mut self, result: Result<(), NodeStartError>, lifecycle: &Lifecycle) {
    if let Some(ready) = self.ready.take() {
      let result = result.and_then(|()| lifecycle.check_ready().map_err(NodeStartError::from));
      let _ = ready.send(result);
    }
  }

  fn timer_finished(lifecycle: &Lifecycle, result: Result<(), JoinError>) {
    match result {
      Err(error) => lifecycle.fail(NodeError::SchedulerStopped(Some(Arc::new(error)))),
      Ok(()) => {
        // Failed initialization can drop the timer's inputs before the worker
        // signals stop. Its startup result reports that failure.
        if lifecycle.status() == NodeStatus::Ready {
          lifecycle.fail(NodeError::SchedulerStopped(None));
        }
      }
    }
  }

  fn service_finished(lifecycle: &Lifecycle, result: Result<(), JoinError>) {
    match result {
      Err(error) => lifecycle.fail(NodeError::IoServiceStopped(Some(Arc::new(error)))),
      Ok(()) if matches!(lifecycle.status(), NodeStatus::Starting | NodeStatus::Ready) => {
        lifecycle.fail(NodeError::IoServiceStopped(None));
      }
      Ok(()) => {}
    }
  }

  async fn supervise(&mut self, lifecycle: &Lifecycle) {
    while self.finished.is_some() {
      tokio::select! {
        biased;
        result = async { self.started.as_mut().expect("enabled startup receiver").await }, if self.started.is_some() => {
          self.started = None;
          let result = result.unwrap_or_else(|_| Err(NodeStartError::Service(lifecycle.unavailable())));
          self.report_start(result, lifecycle);
        }
        result = async { self.timer.as_mut().expect("enabled timer handle").await }, if self.timer.is_some() => {
          self.timer = None;
          Self::timer_finished(lifecycle, result);
        }
        result = async { self.service.as_mut().expect("enabled IO service handle").await }, if self.service.is_some() => {
          self.service = None;
          Self::service_finished(lifecycle, result);
        }
        _ = async { self.finished.as_mut().expect("a running worker has a completion receiver").await } => {
          self.finished = None;
          lifecycle.stop();
        }
      }
    }

    if let Some(started) = self.started.as_mut() {
      let result = started
        .await
        .unwrap_or_else(|_| Err(NodeStartError::Service(lifecycle.unavailable())));
      self.started = None;
      self.report_start(result, lifecycle);
    }
    if let Some(timer) = self.timer.as_mut() {
      let result = timer.await;
      self.timer = None;
      Self::timer_finished(lifecycle, result);
    }
    if let Some(service) = self.service.as_mut() {
      let result = service.await;
      self.service = None;
      Self::service_finished(lifecycle, result);
    }
  }
}

fn run_io<F, Fut>(
  config: NodeConfig,
  requests: mpsc::Sender<Work>,
  receiver: mpsc::Receiver<Work>,
  events: mpsc::UnboundedSender<NodeEvent>,
  ready: oneshot::Sender<Result<(), NodeStartError>>,
  lifecycle: Arc<Lifecycle>,
  io_service: F,
) where
  F: FnOnce() -> Fut,
  Fut: Future<Output = ()> + Send + 'static,
{
  // Keep resource ownership outside the unwind boundary so an IO panic cannot
  // detach a worker that is still waiting on Fork reads.
  let mut runtime = None;
  let mut execution = None;
  let mut progress = IoProgress {
    ready: Some(ready),
    started: None,
    finished: None,
    timer: None,
    service: None,
  };
  let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), NodeStartError> {
    runtime = Some(
      Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(NodeStartError::IoRuntime)?,
    );
    let io = runtime
      .as_ref()
      .expect("IO runtime was just constructed")
      .handle()
      .clone();
    let (schedule, schedules) = watch::channel(None);
    let (started, starting) = oneshot::channel();
    let (finished, finishing) = oneshot::channel();
    let worker_io = io.clone();
    let worker_lifecycle = Arc::clone(&lifecycle);
    execution = Some(
      thread::Builder::new()
        .name("xcfx-execution".into())
        .spawn(move || {
          let result = catch_unwind(AssertUnwindSafe(|| {
            worker::run(
              worker_io,
              config,
              receiver,
              events,
              schedule,
              started,
              Arc::clone(&worker_lifecycle),
            )
          }))
          .unwrap_or_else(|_| {
            worker_lifecycle.fail(NodeError::ExecutionPanicked);
            Err(NodeError::ExecutionPanicked)
          });
          worker_lifecycle.stop();
          let _ = finished.send(());
          result
        })
        .map_err(|source| NodeStartError::Thread {
          thread: "execution",
          source,
        })?,
    );
    progress.started = Some(starting);
    progress.finished = Some(finishing);
    progress.timer = Some(io.spawn(run_timer(requests, schedules, Arc::clone(&lifecycle))));
    progress.service = Some(io.spawn(io_service()));
    runtime
      .as_ref()
      .expect("the IO owner retains its runtime")
      .block_on(progress.supervise(&lifecycle));
    Ok(())
  }));

  match result {
    Ok(Ok(())) => {}
    Ok(Err(error)) => progress.report_start(Err(error), &lifecycle),
    Err(_) => lifecycle.fail(NodeError::IoPanicked),
  }
  lifecycle.stop();

  // A cancelled startup or supervisor panic must still drive the worker's last
  // read and close. Progress is retained across this second unwind boundary.
  if let Some(runtime) = &runtime
    && (progress.finished.is_some()
      || progress.started.is_some()
      || progress.timer.is_some()
      || progress.service.is_some())
    && catch_unwind(AssertUnwindSafe(|| {
      runtime.block_on(progress.supervise(&lifecycle))
    }))
    .is_err()
  {
    lifecycle.fail(NodeError::IoPanicked);
  }

  if progress.finished.is_some() {
    // The IO driver itself is unusable. Cancelling its tasks releases blocked
    // Fork callers before joining the execution thread on this failure path.
    dispose_io(&mut runtime, &lifecycle);
  }
  if let Some(execution) = execution {
    match execution.join() {
      Ok(Ok(())) => {}
      Ok(Err(error)) => lifecycle.fail(error),
      Err(_) => lifecycle.fail(NodeError::ExecutionPanicked),
    }
  }
  dispose_io(&mut runtime, &lifecycle);
  progress.report_start(
    Err(NodeStartError::Service(lifecycle.unavailable())),
    &lifecycle,
  );
  lifecycle.finish(Ok(()));
}

fn dispose_io(runtime: &mut Option<Runtime>, lifecycle: &Lifecycle) {
  if catch_unwind(AssertUnwindSafe(|| drop(runtime.take()))).is_err() {
    lifecycle.fail(NodeError::CleanupPanicked);
  }
}
