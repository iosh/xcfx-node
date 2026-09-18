//! Coalesces remote reads and owns their cache, waiters, and RPC tasks.

use std::{
  collections::{HashMap, hash_map::Entry},
  hash::Hash,
  sync::Arc,
};

use cfx_rpc_cfx_types::Block;
use cfx_types::{AddressWithSpace, H256, U256};
use primitives::block::BlockHeight;
use tokio::{
  runtime::Handle,
  sync::{mpsc, watch},
  task::{JoinError, JoinHandle, JoinSet},
};

use crate::execution::ExecutionCommitment;

use super::{
  ForkEpochReceipts, ForkReadError,
  cache::{ForkCache, ForkCacheConfig},
  client::{ForkClient, ReadRequest, Reply},
  rpc::{ForkRpc, StateKey},
};

// Saturation makes callers wait; these bounds do not limit cumulative reads.
const READ_QUEUE_CAPACITY: usize = 64;
const MAX_PENDING_READERS: usize = 64;

/// Owns service shutdown. Cloned clients cannot stop the service.
pub(crate) struct ForkReadTask {
  stop: watch::Sender<bool>,
  task: Option<JoinHandle<Result<(), JoinError>>>,
}

impl ForkReadTask {
  pub(crate) fn spawn(rpc: ForkRpc, io: &Handle, config: ForkCacheConfig) -> (ForkClient, Self) {
    let base = Arc::clone(&rpc.base);
    let (requests, receiver) = mpsc::channel(READ_QUEUE_CAPACITY);
    let (stop, stopping) = watch::channel(false);
    let service = ForkService {
      rpc: Arc::new(rpc),
      requests: receiver,
      stopping: stopping.clone(),
      cache: ForkCache::new(config),
      pending: PendingReads::default(),
      reads: JoinSet::new(),
    };
    let task = io.spawn(service.run());
    (
      ForkClient::new(base, requests, stopping),
      Self {
        stop,
        task: Some(task),
      },
    )
  }

  pub(crate) fn is_finished(&self) -> bool {
    self.task.as_ref().is_none_or(JoinHandle::is_finished)
  }

  pub(crate) fn request_stop(&self) {
    self.stop.send_replace(true);
  }

  /// Retains the join handle if this wait is cancelled.
  pub(crate) async fn close(&mut self) -> Result<(), JoinError> {
    self.request_stop();
    let Some(task) = self.task.as_mut() else {
      return Ok(());
    };
    let result = match task.await {
      Ok(result) => result,
      Err(error) => Err(error),
    };
    self.task = None;
    result
  }
}

impl Drop for ForkReadTask {
  fn drop(&mut self) {
    self.request_stop();
  }
}

struct ForkService {
  rpc: Arc<ForkRpc>,
  requests: mpsc::Receiver<ReadRequest>,
  stopping: watch::Receiver<bool>,
  cache: ForkCache,
  pending: PendingReads,
  reads: JoinSet<CompletedRead>,
}

impl ForkService {
  async fn run(mut self) -> Result<(), JoinError> {
    let outcome = loop {
      let accepting = self.pending.waiter_count < MAX_PENDING_READERS;
      tokio::select! {
        biased;
        // Keep the select output from borrowing the watch receiver.
        _ = async {
          drop(self.stopping.wait_for(|stopped| *stopped).await);
        } => break Ok(()),
        completed = self.reads.join_next(), if !self.reads.is_empty() => {
          let completed = match completed.expect("a nonempty read set must yield a task") {
            Ok(completed) => completed,
            Err(error) => break Err(error),
          };
          self.complete_read(completed);
        }
        request = self.requests.recv(), if accepting => {
          let Some(request) = request else {
            break Ok(());
          };
          self.start_read(request);
        }
      }
    };

    // Release blocked senders and reply receivers before waiting for RPC cleanup.
    drop(self.requests);
    drop(self.pending);
    self.reads.shutdown().await;
    outcome
  }

  fn start_read(&mut self, request: ReadRequest) {
    match request {
      ReadRequest::Balance { address, reply } => {
        if let Some(value) = self.cache.balance(&address) {
          let _ = reply.send(Ok(value));
          return;
        }
        let should_start_rpc = register_waiter(
          &mut self.pending.balances,
          &mut self.pending.waiter_count,
          address,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.balance(address).await;
            CompletedRead::Balance { address, result }
          });
        }
      }
      ReadRequest::StateValue { key, reply } => {
        if let Some(value) = self.cache.state_value(&key) {
          let _ = reply.send(Ok(value));
          return;
        }
        let should_start_rpc = register_waiter(
          &mut self.pending.state_values,
          &mut self.pending.waiter_count,
          key,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.state_value(key).await;
            CompletedRead::StateValue { key, result }
          });
        }
      }
      ReadRequest::Pivot { epoch, reply } => {
        let should_start_rpc = register_waiter(
          &mut self.pending.pivots,
          &mut self.pending.waiter_count,
          epoch,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.pivot(epoch).await;
            CompletedRead::Pivot { epoch, result }
          });
        }
      }
      ReadRequest::Block { epoch, hash, reply } => {
        let should_start_rpc = register_waiter(
          &mut self.pending.blocks,
          &mut self.pending.waiter_count,
          (epoch, hash),
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.block(epoch, hash).await;
            CompletedRead::Block {
              epoch,
              hash,
              result,
            }
          });
        }
      }
      ReadRequest::BlockHashes { epoch, reply } => {
        let should_start_rpc = register_waiter(
          &mut self.pending.block_hashes,
          &mut self.pending.waiter_count,
          epoch,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.block_hashes(epoch).await;
            CompletedRead::BlockHashes { epoch, result }
          });
        }
      }
      ReadRequest::Receipts {
        epoch,
        include_espace,
        reply,
      } => {
        let should_start_rpc = register_waiter(
          &mut self.pending.receipts,
          &mut self.pending.waiter_count,
          (epoch, include_espace),
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.receipts(epoch, include_espace).await;
            CompletedRead::Receipts {
              epoch,
              include_espace,
              result,
            }
          });
        }
      }
      ReadRequest::Commitment { epoch, reply } => {
        if let Some(value) = self.cache.commitment(epoch) {
          let _ = reply.send(Ok(value));
          return;
        }
        let should_start_rpc = register_waiter(
          &mut self.pending.commitments,
          &mut self.pending.waiter_count,
          epoch,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.commitment(epoch).await;
            CompletedRead::Commitment { epoch, result }
          });
        }
      }
    }
  }

  fn complete_read(&mut self, completed: CompletedRead) {
    match completed {
      CompletedRead::Balance { address, result } => {
        if let Ok(value) = &result {
          self.cache.insert_balance(address, *value);
        }
        reply_to_waiters(
          &mut self.pending.balances,
          &mut self.pending.waiter_count,
          &address,
          result,
        );
      }
      CompletedRead::StateValue { key, result } => {
        if let Ok(value) = &result {
          self.cache.insert_state_value(key, value.clone());
        }
        reply_to_waiters(
          &mut self.pending.state_values,
          &mut self.pending.waiter_count,
          &key,
          result,
        );
      }
      CompletedRead::Pivot { epoch, result } => {
        reply_to_waiters(
          &mut self.pending.pivots,
          &mut self.pending.waiter_count,
          &epoch,
          result,
        );
      }
      CompletedRead::Block {
        epoch,
        hash,
        result,
      } => {
        reply_to_waiters(
          &mut self.pending.blocks,
          &mut self.pending.waiter_count,
          &(epoch, hash),
          result,
        );
      }
      CompletedRead::BlockHashes { epoch, result } => {
        reply_to_waiters(
          &mut self.pending.block_hashes,
          &mut self.pending.waiter_count,
          &epoch,
          result,
        );
      }
      CompletedRead::Receipts {
        epoch,
        include_espace,
        result,
      } => {
        reply_to_waiters(
          &mut self.pending.receipts,
          &mut self.pending.waiter_count,
          &(epoch, include_espace),
          result,
        );
      }
      CompletedRead::Commitment { epoch, result } => {
        if let Ok(value) = &result {
          self.cache.insert_commitment(epoch, value.clone());
        }
        reply_to_waiters(
          &mut self.pending.commitments,
          &mut self.pending.waiter_count,
          &epoch,
          result,
        );
      }
    }
  }
}

enum CompletedRead {
  Balance {
    address: AddressWithSpace,
    result: Result<U256, ForkReadError>,
  },
  StateValue {
    key: StateKey,
    result: Result<Option<Arc<[u8]>>, ForkReadError>,
  },
  Pivot {
    epoch: BlockHeight,
    result: Result<Arc<Block>, ForkReadError>,
  },
  Block {
    epoch: BlockHeight,
    hash: H256,
    result: Result<Arc<Block>, ForkReadError>,
  },
  BlockHashes {
    epoch: BlockHeight,
    result: Result<Arc<[H256]>, ForkReadError>,
  },
  Receipts {
    epoch: BlockHeight,
    include_espace: bool,
    result: Result<Arc<ForkEpochReceipts>, ForkReadError>,
  },
  Commitment {
    epoch: BlockHeight,
    result: Result<ExecutionCommitment, ForkReadError>,
  },
}

#[derive(Default)]
struct PendingReads {
  waiter_count: usize,
  balances: HashMap<AddressWithSpace, Vec<Reply<U256>>>,
  state_values: HashMap<StateKey, Vec<Reply<Option<Arc<[u8]>>>>>,
  pivots: HashMap<BlockHeight, Vec<Reply<Arc<Block>>>>,
  blocks: HashMap<(BlockHeight, H256), Vec<Reply<Arc<Block>>>>,
  block_hashes: HashMap<BlockHeight, Vec<Reply<Arc<[H256]>>>>,
  receipts: HashMap<(BlockHeight, bool), Vec<Reply<Arc<ForkEpochReceipts>>>>,
  commitments: HashMap<BlockHeight, Vec<Reply<ExecutionCommitment>>>,
}

// Only the first waiter for a key starts RPC; every waiter occupies capacity.
fn register_waiter<K: Eq + Hash, T>(
  pending: &mut HashMap<K, Vec<Reply<T>>>,
  waiter_count: &mut usize,
  key: K,
  reply: Reply<T>,
) -> bool {
  if reply.is_closed() {
    return false;
  }
  *waiter_count += 1;
  match pending.entry(key) {
    Entry::Occupied(mut entry) => {
      entry.get_mut().push(reply);
      false
    }
    Entry::Vacant(entry) => {
      entry.insert(vec![reply]);
      true
    }
  }
}

fn reply_to_waiters<K: Eq + Hash, T: Clone>(
  pending: &mut HashMap<K, Vec<Reply<T>>>,
  waiter_count: &mut usize,
  key: &K,
  result: Result<T, ForkReadError>,
) {
  let replies = pending
    .remove(key)
    .expect("a completed fork read must have registered waiters");
  *waiter_count -= replies.len();
  for reply in replies {
    let _ = reply.send(result.clone());
  }
}
