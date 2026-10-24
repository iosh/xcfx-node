//! Owns the Runtime and executes complete messages without interleaving.

use std::{
  panic::{AssertUnwindSafe, catch_unwind},
  sync::Arc,
  time::Duration,
};

use cfx_rpc_eth_types::{Receipt as EthereumReceipt, Transaction as EthereumTransaction};
use cfx_types::{Address, AddressWithSpace, H256, Space, U256};
use primitives::BlockNumber;
use tokio::{
  runtime::Handle,
  sync::{mpsc, oneshot, watch},
};

use crate::{
  chain::{EspaceBlockView, EspaceTransactionView, MinedBlockView, TransactionReceiptView},
  config::{MiningMode, NodeConfig},
  fork::ForkReadError,
  production_environment::BlockTimestamp,
  rpc_client::EthereumBlock,
  runtime::{
    AccountPoolTransactions, EpochSelector, EspaceBlockSelector, EspaceBlockSource,
    EspaceReceiptSource, EspaceStateQueryError, EspaceStateSelector, EspaceTransactionSource,
    LocalEspaceTransactionSource, RuntimeCommitOutcome, RuntimeTransactionError, StateQueryError,
  },
  runtime_transaction::RuntimeTransaction,
  state_overlay::{StateControlBatch, StateControlPreparationError},
};

use super::{
  Node, NodeError, NodeEvent, NodeStartError,
  lifecycle::{Lifecycle, NodeStatus},
  mining::{
    InvalidMiningInterval, MineRequest, MiningCause, MiningError, MiningOutcome, MiningScheduler,
    Tick, TimerSchedule,
  },
  request::{CallId, Cancellation, Message, MessageId, RequestError, Work},
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
    if worker.lifecycle.mark_ready() {
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
      message.reject(lifecycle.unavailable().into());
    }
  }
}

pub(super) struct Worker<'a> {
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
        Some(Work::Message(message)) => message.execute(self),
        Some(Work::Tick(tick)) => self.execute_tick(tick),
        None => return,
      }
    }
  }

  pub(super) fn execute_message<T>(&mut self, message: Message<T>) {
    let Message {
      id,
      calls,
      cancellation,
      reply,
    } = message;
    let count = calls.len();
    // Completed values and the only reply sender survive an unexpected unwind
    // of the batch loop or automatic mining, independently of each task's boundary.
    let mut results = Vec::with_capacity(count);
    let mut auto_mine = false;
    let execution = catch_unwind(AssertUnwindSafe(|| {
      for call in calls {
        let call_id = CallId {
          message: id,
          index: call.index,
        };
        let result = self.execute_call(&cancellation, &call.cancellation, |worker| {
          (call.task)(NodeCall {
            worker,
            id: call_id,
            message_cancel: &cancellation,
            call_cancel: &call.cancellation,
            auto_mine: &mut auto_mine,
          })
        });
        results.push(result);
      }
    }));
    if execution.is_err() {
      self.lifecycle.fail(NodeError::ExecutionPanicked);
      results.resize_with(count, || Err(NodeError::ExecutionPanicked.into()));
    }
    if auto_mine && self.scheduler.mode() == MiningMode::Auto {
      let mining = catch_unwind(AssertUnwindSafe(|| self.auto_mine(id, &cancellation)));
      if mining.is_err() {
        self.lifecycle.fail(NodeError::ExecutionPanicked);
        self.mining_failed(
          MiningCause::Automatic(id),
          RequestError::Node(NodeError::ExecutionPanicked).into(),
        );
      }
    }
    let _ = reply.send(Ok(results));
  }

  /// The batch retains response ownership while the task consumes its node access.
  fn execute_call<T>(
    &mut self,
    message_cancel: &Cancellation,
    call_cancel: &Cancellation,
    operation: impl FnOnce(&mut Self) -> T,
  ) -> Result<T, RequestError> {
    self.lifecycle.begin(&[message_cancel, call_cancel])?;
    self.check_health()?;
    let result = catch_unwind(AssertUnwindSafe(|| operation(self)));
    match result {
      Ok(result) => {
        // Preserve completed command results even if an independent IO task
        // failed during the operation. Subsequent operations see that failure.
        if self.lifecycle.status() != NodeStatus::Failed {
          let _ = self.check_health();
        }
        Ok(result)
      }
      Err(_) => {
        self.lifecycle.fail(NodeError::ExecutionPanicked);
        Err(RequestError::Node(NodeError::ExecutionPanicked))
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
    self.check_health()?;
    let result = catch_unwind(AssertUnwindSafe(|| {
      self
        .node
        .runtime_mut()
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
      && let Err(error) = self.scheduler.rearm()
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

/// A receipt captured during one node operation. Local results pin their epoch;
/// remote results have already passed the fixed-prefix history checks.
#[derive(Clone)]
pub(crate) enum EspaceReceipt {
  Local(EspaceTransactionView),
  Fork(Arc<EthereumReceipt>),
}

pub(crate) enum EspaceBlock {
  Local(EspaceBlockView),
  Fork(Arc<EthereumBlock>),
}

pub(crate) enum EspaceTransaction {
  Pool(RuntimeTransaction),
  Local(EspaceTransactionView),
  Fork(Arc<EthereumTransaction>),
}

pub(crate) enum EspaceBlockReceipts {
  Local(EspaceBlockView),
  Fork(Arc<[EthereumReceipt]>),
}

fn capture_local_espace_transaction(source: LocalEspaceTransactionSource<'_>) -> EspaceTransaction {
  match source {
    LocalEspaceTransactionSource::Pool(transaction) => EspaceTransaction::Pool(transaction.clone()),
    LocalEspaceTransactionSource::Mined(transaction) => EspaceTransaction::Local(transaction),
  }
}

fn capture_espace_transaction(
  source: Option<EspaceTransactionSource<'_>>,
) -> Result<Option<EspaceTransaction>, ForkReadError> {
  match source {
    None => Ok(None),
    Some(EspaceTransactionSource::Local(source)) => {
      Ok(Some(capture_local_espace_transaction(source)))
    }
    Some(EspaceTransactionSource::Fork { client, hash }) => Ok(
      client
        .espace_transaction_by_hash(hash)?
        .map(EspaceTransaction::Fork),
    ),
  }
}

/// Access to one operation after its cancellation and health checks have passed.
/// Methods consume this capability so a task cannot start another operation
/// without going through the worker's boundaries again. No Runtime borrow escapes.
pub(crate) struct NodeCall<'call, 'worker> {
  worker: &'call mut Worker<'worker>,
  id: CallId,
  message_cancel: &'call Cancellation,
  call_cancel: &'call Cancellation,
  auto_mine: &'call mut bool,
}

impl NodeCall<'_, '_> {
  /// Orders one connection observation change against this thread's facts.
  /// The IO consumer applies it before processing any subsequent fact.
  pub(crate) fn observation_boundary(self) {
    let _ = self
      .worker
      .events
      .send(NodeEvent::Observation { call: self.id });
  }

  pub(crate) fn espace_chain_id(self) -> u32 {
    self.worker.node.runtime().espace_chain_id()
  }

  pub(crate) fn epoch_height(self, epoch: EpochSelector) -> u64 {
    self.worker.node.runtime().epoch_height(epoch)
  }

  pub(crate) fn balance_at(
    self,
    address: AddressWithSpace,
    epoch: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    self.worker.node.runtime().balance_at(address, epoch)
  }

  pub(crate) fn espace_balance_at(
    self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<U256, EspaceStateQueryError> {
    self
      .worker
      .node
      .runtime()
      .espace_balance_at(address, selector)
  }

  pub(crate) fn espace_nonce_at(
    self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<U256, EspaceStateQueryError> {
    self
      .worker
      .node
      .runtime()
      .espace_nonce_at(address, selector)
  }

  pub(crate) fn espace_pending_nonce(
    self,
    address: Address,
  ) -> Result<U256, EspaceStateQueryError> {
    self.worker.node.runtime().espace_pending_nonce(address)
  }

  pub(crate) fn espace_code_at(
    self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<Arc<Vec<u8>>, EspaceStateQueryError> {
    self.worker.node.runtime().espace_code_at(address, selector)
  }

  pub(crate) fn espace_storage_at(
    self,
    address: Address,
    position: H256,
    selector: EspaceStateSelector,
  ) -> Result<H256, EspaceStateQueryError> {
    self
      .worker
      .node
      .runtime()
      .espace_storage_at(address, position, selector)
  }

  pub(crate) fn espace_block(
    self,
    selector: EspaceBlockSelector,
    full_transactions: bool,
  ) -> Result<Option<EspaceBlock>, ForkReadError> {
    match self.worker.node.runtime().espace_block_source(selector) {
      None => Ok(None),
      Some(EspaceBlockSource::Local(block)) => Ok(Some(EspaceBlock::Local(block))),
      Some(EspaceBlockSource::Fork { client, selector }) => Ok(
        client
          .espace_block(selector, full_transactions)?
          .map(EspaceBlock::Fork),
      ),
    }
  }

  pub(crate) fn espace_transaction_by_hash(
    self,
    hash: H256,
  ) -> Result<Option<EspaceTransaction>, ForkReadError> {
    capture_espace_transaction(self.worker.node.runtime().espace_transaction_source(hash))
  }

  pub(crate) fn espace_transaction_by_sender_and_nonce(
    self,
    address: Address,
    nonce: U256,
  ) -> Option<EspaceTransaction> {
    self
      .worker
      .node
      .runtime()
      .espace_transaction_by_sender_and_nonce(address, nonce)
      .map(capture_local_espace_transaction)
  }

  pub(crate) fn espace_transaction_by_block_and_index(
    self,
    selector: EspaceBlockSelector,
    index: usize,
  ) -> Result<Option<EspaceTransaction>, ForkReadError> {
    match self.worker.node.runtime().espace_block_source(selector) {
      None => Ok(None),
      Some(EspaceBlockSource::Local(block)) => {
        Ok(block.transaction_at(index).map(EspaceTransaction::Local))
      }
      Some(EspaceBlockSource::Fork { client, selector }) => Ok(
        client
          .espace_transaction_by_block_and_index(selector, index)?
          .map(EspaceTransaction::Fork),
      ),
    }
  }

  pub(crate) fn espace_block_receipts(
    self,
    selector: EspaceBlockSelector,
  ) -> Result<Option<EspaceBlockReceipts>, ForkReadError> {
    match self.worker.node.runtime().espace_block_source(selector) {
      None => Ok(None),
      Some(EspaceBlockSource::Local(block)) => Ok(Some(EspaceBlockReceipts::Local(block))),
      Some(EspaceBlockSource::Fork { client, selector }) => Ok(
        client
          .espace_block_receipts(selector)?
          .map(EspaceBlockReceipts::Fork),
      ),
    }
  }

  pub(crate) fn mined_block_by_number(self, number: BlockNumber) -> Option<MinedBlockView> {
    self.worker.node.runtime().mined_block_by_number(number)
  }

  pub(crate) fn pool_transaction_by_hash(self, hash: H256) -> Option<RuntimeTransaction> {
    self
      .worker
      .node
      .runtime()
      .pool_transaction_by_hash(hash)
      .cloned()
  }

  /// Captures owned pool results before automatic mining or another message can run.
  pub(crate) fn pool_account_transactions(
    self,
    address: AddressWithSpace,
    start_nonce: Option<U256>,
    limit: usize,
  ) -> cfx_statedb::Result<AccountPoolTransactions> {
    self
      .worker
      .node
      .runtime()
      .pool_account_transactions(address, start_nonce, limit)
  }

  /// Looks up a local receipt under the Runtime's deferred visibility rules.
  pub(crate) fn transaction_receipt_by_hash(self, hash: &H256) -> Option<TransactionReceiptView> {
    self.worker.node.runtime().transaction_receipt_by_hash(hash)
  }

  /// Completes pool, local, and fixed-prefix receipt lookup before returning.
  /// The captured result is independent of later mining or history changes.
  ///
  /// # Errors
  /// Propagates remote read failures; an unavailable local receipt never triggers IO.
  pub(crate) fn espace_transaction_receipt_by_hash(
    self,
    hash: &H256,
  ) -> Result<Option<EspaceReceipt>, ForkReadError> {
    match self.worker.node.runtime().espace_receipt_source(hash) {
      None => Ok(None),
      Some(EspaceReceiptSource::Local(transaction)) => Ok(Some(EspaceReceipt::Local(transaction))),
      Some(EspaceReceiptSource::Fork { client, hash }) => {
        Ok(client.espace_receipt(hash)?.map(EspaceReceipt::Fork))
      }
    }
  }

  pub(crate) fn submit_raw_transaction(self, raw: &[u8]) -> Result<H256, RuntimeTransactionError> {
    let hash = self.worker.node.runtime_mut().submit_raw_transaction(raw)?;
    *self.auto_mine = true;
    Ok(hash)
  }

  pub(crate) fn apply_state_controls(
    self,
    batch: &StateControlBatch,
  ) -> Result<(), StateControlPreparationError> {
    self
      .worker
      .node
      .runtime_mut()
      .apply_state_control_batch(batch)?;
    *self.auto_mine |= !batch.is_empty();
    Ok(())
  }

  pub(crate) fn mine(self, request: MineRequest) -> MiningOutcome {
    self
      .worker
      .mine(request, self.id, self.message_cancel, self.call_cancel)
  }

  pub(crate) fn mining_mode(self) -> MiningMode {
    self.worker.scheduler.mode()
  }

  pub(crate) fn set_automine(self, enabled: bool) -> MiningMode {
    let previous = self.worker.scheduler.mode();
    self.worker.scheduler.set_automine(enabled);
    let mode = self.worker.scheduler.mode();
    *self.auto_mine |= previous != MiningMode::Auto && mode == MiningMode::Auto;
    mode
  }

  /// Zero disables interval mining; positive values select interval mode.
  pub(crate) fn set_interval_mining(
    self,
    interval: Duration,
  ) -> Result<MiningMode, InvalidMiningInterval> {
    self.worker.scheduler.set_interval(interval)?;
    Ok(self.worker.scheduler.mode())
  }
}
