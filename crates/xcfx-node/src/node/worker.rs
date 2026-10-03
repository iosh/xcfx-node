//! Owns the Runtime and executes complete messages without interleaving.

use std::{
  panic::{AssertUnwindSafe, catch_unwind},
  sync::Arc,
};

use tokio::{
  runtime::Handle,
  sync::{mpsc, oneshot, watch},
};

use crate::{
  config::{MiningMode, NodeConfig},
  production_environment::BlockTimestamp,
  runtime::RuntimeCommitOutcome,
};

use super::{
  Node, NodeError, NodeStartError,
  command::{
    CallId, Cancellation, CommandError, CommandOutcome, Message, MessageId, MessageOutcome,
    MineRequest, MiningError, MiningOutcome, NodeCommand, RequestError, Work,
  },
  lifecycle::{Lifecycle, NodeStatus},
  mining::{MiningCause, MiningScheduler, NodeEvent, Tick, TimerSchedule},
};

pub(super) fn run(
  io: Handle,
  config: NodeConfig,
  mut requests: mpsc::Receiver<Work>,
  events: mpsc::UnboundedSender<NodeEvent>,
  schedule: watch::Sender<Option<TimerSchedule>>,
  started: oneshot::Sender<Result<(), NodeStartError>>,
  lifecycle: Arc<Lifecycle>,
) -> Result<(), NodeError> {
  let mode = config.mining;
  let startup = catch_unwind(AssertUnwindSafe(|| Node::start(io.clone(), config)));
  let mut node = match startup {
    Ok(Ok(node)) => node,
    Ok(Err(error)) => {
      lifecycle.stop();
      let _ = started.send(Err(error));
      return Ok(());
    }
    Err(_) => {
      lifecycle.fail(NodeError::ExecutionPanicked);
      let _ = started.send(Err(NodeError::ExecutionPanicked.into()));
      return Err(NodeError::ExecutionPanicked);
    }
  };
  // Keep the node and queued replies outside the unwind boundary. A failed
  // command loop must not discard the resources needed for orderly shutdown.
  let result = catch_unwind(AssertUnwindSafe(|| {
    // Retain the original sender until stop is recorded. Dropping a panicked
    // worker must not make the timer report an unrelated scheduler failure.
    let scheduler = match MiningScheduler::new(mode, schedule.clone()) {
      Ok(scheduler) => scheduler,
      Err(error) => {
        lifecycle.stop();
        let _ = started.send(Err(error.into()));
        return;
      }
    };
    let mut worker = Worker {
      node: &mut node,
      scheduler,
      requests: &mut requests,
      events: &events,
      lifecycle: &lifecycle,
      io: &io,
    };
    if worker.lifecycle.ready() {
      worker.scheduler.publish_schedule();
      let _ = started.send(Ok(()));
      worker.run();
    } else {
      let _ = started.send(Err(worker.lifecycle.unavailable().into()));
    }
  }));
  if result.is_err() {
    lifecycle.fail(NodeError::ExecutionPanicked);
  }
  lifecycle.stop();
  if catch_unwind(AssertUnwindSafe(|| {
    reject_queued(&mut requests, &lifecycle)
  }))
  .is_err()
  {
    lifecycle.fail(NodeError::ExecutionPanicked);
  }
  let result =
    catch_unwind(AssertUnwindSafe(|| node.close())).unwrap_or(Err(NodeError::CleanupPanicked));
  if let Err(error) = &result {
    lifecycle.fail(error.clone());
  }
  result
}

fn reject_queued(requests: &mut mpsc::Receiver<Work>, lifecycle: &Lifecycle) {
  requests.close();
  // Stop and admission share a lock, so no outstanding permit may publish
  // after stop. Do not await those permits: a suspended producer may retain one.
  while let Ok(work) = requests.try_recv() {
    if let Work::Message(message) = work {
      let error = lifecycle.unavailable();
      let outcomes = message
        .calls
        .into_iter()
        .map(|_| Err(RequestError::Node(error.clone()).into()))
        .collect();
      let _ = message.reply.send(MessageOutcome {
        id: message.id,
        calls: outcomes,
      });
    }
  }
}

struct Worker<'a> {
  node: &'a mut Node,
  scheduler: MiningScheduler,
  requests: &'a mut mpsc::Receiver<Work>,
  events: &'a mpsc::UnboundedSender<NodeEvent>,
  lifecycle: &'a Lifecycle,
  io: &'a Handle,
}

impl Worker<'_> {
  fn run(&mut self) {
    loop {
      let work = self.io.block_on(async {
        tokio::select! {
          biased;
          _ = self.lifecycle.stopped() => None,
          work = self.requests.recv() => work,
        }
      });
      // All synchronous Runtime calls run after leaving the async context.
      match work {
        Some(Work::Message(message)) => self.execute_message(message),
        Some(Work::Tick(tick)) => self.execute_tick(tick),
        None => return,
      }
    }
  }

  fn execute_message(&mut self, message: Message) {
    let Message {
      id,
      calls,
      cancellation,
      reply,
    } = message;
    let mut outcomes = Vec::with_capacity(calls.len());
    let mut auto_mine = false;
    for (index, call) in calls.into_iter().enumerate() {
      let triggers_mining = match &call.command {
        NodeCommand::SubmitRawTransaction(_) => true,
        NodeCommand::ApplyStateControls(batch) => !batch.is_empty(),
        NodeCommand::SetAutomine(true) => self.scheduler.mode() != MiningMode::Auto,
        _ => false,
      };
      let call_id = CallId { message: id, index };
      let outcome = self.execute_call(call.command, call_id, &cancellation, &call.cancellation);
      auto_mine |= triggers_mining && outcome.is_ok();
      outcomes.push(outcome);
    }
    if auto_mine && self.scheduler.mode() == MiningMode::Auto {
      self.auto_mine(id, &cancellation);
    }
    let _ = reply.send(MessageOutcome {
      id,
      calls: outcomes,
    });
  }

  fn execute_call(
    &mut self,
    command: NodeCommand,
    id: CallId,
    message_cancel: &Cancellation,
    call_cancel: &Cancellation,
  ) -> Result<CommandOutcome, CommandError> {
    self.lifecycle.begin(&[message_cancel, call_cancel])?;
    self.check_health()?;
    let result = catch_unwind(AssertUnwindSafe(|| {
      self.dispatch(command, id, message_cancel, call_cancel)
    }));
    match result {
      Ok(result) => {
        // Preserve completed command results even if an independent IO task
        // failed during the operation. Subsequent operations see that failure.
        if self.lifecycle.status() != NodeStatus::Failed {
          let _ = self.check_health();
        }
        result
      }
      Err(_) => {
        self.lifecycle.fail(NodeError::ExecutionPanicked);
        Err(RequestError::Node(NodeError::ExecutionPanicked).into())
      }
    }
  }

  fn dispatch(
    &mut self,
    command: NodeCommand,
    id: CallId,
    message_cancel: &Cancellation,
    call_cancel: &Cancellation,
  ) -> Result<CommandOutcome, CommandError> {
    match command {
      NodeCommand::EpochHeight(epoch) => Ok(CommandOutcome::EpochHeight(
        self
          .node
          .runtime()
          .map_err(RequestError::from)?
          .epoch_height(epoch),
      )),
      NodeCommand::Balance { address, epoch } => Ok(CommandOutcome::Balance(
        self
          .node
          .runtime()
          .map_err(RequestError::from)?
          .balance_at(address, epoch)?,
      )),
      NodeCommand::TransactionReceipt(hash) => Ok(CommandOutcome::TransactionReceipt(
        self
          .node
          .runtime()
          .map_err(RequestError::from)?
          .transaction_receipt_by_hash(&hash),
      )),
      NodeCommand::SubmitRawTransaction(raw) => {
        let hash = self
          .node
          .runtime_mut()
          .map_err(RequestError::from)?
          .submit_raw_transaction(&raw)?;
        Ok(CommandOutcome::TransactionSubmitted(hash))
      }
      NodeCommand::ApplyStateControls(batch) => {
        self
          .node
          .runtime_mut()
          .map_err(RequestError::from)?
          .apply_state_control_batch(&batch)?;
        Ok(CommandOutcome::StateControlsApplied)
      }
      NodeCommand::Mine(request) => Ok(CommandOutcome::Mined(self.mine(
        request,
        id,
        message_cancel,
        call_cancel,
      ))),
      NodeCommand::MiningMode => Ok(CommandOutcome::MiningMode(self.scheduler.mode())),
      NodeCommand::SetAutomine(enabled) => {
        self.scheduler.set_automine(enabled);
        Ok(CommandOutcome::MiningMode(self.scheduler.mode()))
      }
      NodeCommand::SetIntervalMining(interval) => {
        self.scheduler.set_interval(interval)?;
        Ok(CommandOutcome::MiningMode(self.scheduler.mode()))
      }
    }
  }

  fn check_health(&self) -> Result<(), RequestError> {
    self.node.check_read_service().map_err(|error| {
      self.lifecycle.fail(error.clone());
      error.into()
    })
  }

  fn mine(
    &mut self,
    request: MineRequest,
    id: CallId,
    message_cancel: &Cancellation,
    call_cancel: &Cancellation,
  ) -> MiningOutcome {
    let mut hashes = Vec::new();
    for index in 0..request.blocks {
      if let Err(error) = self.lifecycle.begin(&[message_cancel, call_cancel]) {
        return MiningOutcome::Interrupted {
          hashes,
          reason: error.into(),
        };
      }
      match self.mine_block(request.timing.for_block(index)) {
        Ok(outcome) => {
          hashes.push(outcome.optimistic_head.state().epoch_id);
          self.committed(MiningCause::Manual(id), outcome);
        }
        Err(reason) => return MiningOutcome::Interrupted { hashes, reason },
      }
    }
    MiningOutcome::Complete(hashes)
  }

  fn mine_block(&mut self, timestamp: BlockTimestamp) -> Result<RuntimeCommitOutcome, MiningError> {
    let result = catch_unwind(AssertUnwindSafe(|| {
      let runtime = self.node.runtime_mut().map_err(RequestError::from)?;
      runtime
        .produce_and_commit_block(timestamp)
        .map_err(MiningError::from)
    }));
    match result {
      Ok(result) => {
        if self.lifecycle.status() != NodeStatus::Failed {
          let _ = self.check_health();
        }
        result
      }
      Err(_) => {
        self.lifecycle.fail(NodeError::ExecutionPanicked);
        Err(RequestError::Node(NodeError::ExecutionPanicked).into())
      }
    }
  }

  fn auto_mine(&mut self, id: MessageId, cancellation: &Cancellation) {
    if self.lifecycle.begin(&[cancellation]).is_err() {
      return;
    }
    let cause = MiningCause::Automatic(id);
    if let Err(error) = self.check_health() {
      self.mining_failed(cause, error.into());
      return;
    }
    let Node {
      runtime, read_task, ..
    } = &mut *self.node;
    // Borrow the read-service health separately, keeping the iterator's exclusive
    // Runtime borrow for the entire run. No other command may extend its work set.
    let mut mining = runtime.auto_mine();
    loop {
      if self.lifecycle.begin(&[cancellation]).is_err() {
        break;
      }
      if read_task.as_ref().is_some_and(|task| task.is_finished()) {
        let error = NodeError::ForkServiceStopped(None);
        self.lifecycle.fail(error.clone());
        let _ = self.events.send(NodeEvent::MiningFailed {
          cause,
          error: Arc::new(RequestError::Node(error).into()),
        });
        break;
      }
      match catch_unwind(AssertUnwindSafe(|| mining.next())) {
        Ok(Some(Ok(outcome))) => {
          let _ = self.events.send(NodeEvent::Committed {
            cause,
            outcome: Arc::new(outcome),
          });
        }
        Ok(Some(Err(error))) => {
          let _ = self.events.send(NodeEvent::MiningFailed {
            cause,
            error: Arc::new(error.into()),
          });
          break;
        }
        Ok(None) => break,
        Err(_) => {
          self.lifecycle.fail(NodeError::ExecutionPanicked);
          let error = RequestError::Node(NodeError::ExecutionPanicked).into();
          let _ = self.events.send(NodeEvent::MiningFailed {
            cause,
            error: Arc::new(error),
          });
          break;
        }
      }
    }
    if self.lifecycle.status() != NodeStatus::Failed {
      let _ = self.check_health();
    }
  }

  fn execute_tick(&mut self, tick: Tick) {
    if !self.scheduler.accepts(&tick) || self.lifecycle.begin(&[]).is_err() {
      return;
    }
    let cause = MiningCause::Interval;
    match self.mine_block(BlockTimestamp::Default) {
      Ok(outcome) => self.committed(cause, outcome),
      Err(error) => self.mining_failed(cause, error),
    }
    if self.lifecycle.status() == NodeStatus::Ready
      && let Err(error) = self.scheduler.rearm(&tick)
    {
      self.mining_failed(cause, error.into());
    }
  }

  fn committed(&self, cause: MiningCause, outcome: RuntimeCommitOutcome) {
    let _ = self.events.send(NodeEvent::Committed {
      cause,
      outcome: Arc::new(outcome),
    });
  }

  fn mining_failed(&self, cause: MiningCause, error: MiningError) {
    let _ = self.events.send(NodeEvent::MiningFailed {
      cause,
      error: Arc::new(error),
    });
  }
}
