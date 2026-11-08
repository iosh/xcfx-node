//! Coalesces remote reads and owns their cache, waiters, and RPC tasks.

use std::{
  collections::{HashMap, hash_map::Entry},
  hash::Hash,
  sync::Arc,
};

use cfx_rpc_cfx_types::Block;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, U256};
use primitives::block::BlockHeight;
use tokio::{
  runtime::Handle,
  sync::{mpsc, watch},
  task::{JoinError, JoinHandle, JoinSet},
};

use crate::execution::ExecutionCommitment;

use super::{
  ForkEpochReceipts, ForkReadError, HistoryQuery, HistoryResult, StateValue,
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
      ReadRequest::History { query, reply } => {
        let should_start_rpc = register_waiter(
          &mut self.pending.history,
          &mut self.pending.waiter_count,
          query.clone(),
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.history(query.clone()).await;
            CompletedRead::History { query, result }
          });
        }
      }
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
      ReadRequest::StoragePointsInitialization { address, reply } => {
        if let Some(value) = self.cache.storage_points_initialized(&address) {
          let _ = reply.send(Ok(value));
          return;
        }
        let account = match self
          .cache
          .state_value(&StateKey::Account(address.with_native_space()))
        {
          Some(Some(StateValue::Account(account))) => {
            if let Some(value) = account.sponsor_info.storage_points.initialization() {
              let _ = reply.send(Ok(value));
              return;
            }
            Some(account)
          }
          Some(None) => {
            let _ = reply.send(Ok(false));
            return;
          }
          None => None,
          Some(Some(_)) => unreachable!("an account cache key must contain account data"),
        };
        let should_start_rpc = register_waiter(
          &mut self.pending.storage_points_initializations,
          &mut self.pending.waiter_count,
          address,
          reply,
        );
        if should_start_rpc {
          let rpc = Arc::clone(&self.rpc);
          self.reads.spawn(async move {
            let result = rpc.storage_points_initialized(address, account).await;
            CompletedRead::StoragePointsInitialization { address, result }
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
      CompletedRead::History { query, result } => {
        reply_to_waiters(
          &mut self.pending.history,
          &mut self.pending.waiter_count,
          &query,
          result,
        );
      }
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
      CompletedRead::StoragePointsInitialization { address, result } => {
        if let Ok(value) = &result {
          self
            .cache
            .insert_storage_points_initialized(address, *value);
        }
        reply_to_waiters(
          &mut self.pending.storage_points_initializations,
          &mut self.pending.waiter_count,
          &address,
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
  History {
    query: HistoryQuery,
    result: Result<HistoryResult, ForkReadError>,
  },
  Balance {
    address: AddressWithSpace,
    result: Result<U256, ForkReadError>,
  },
  StateValue {
    key: StateKey,
    result: Result<Option<StateValue>, ForkReadError>,
  },
  StoragePointsInitialization {
    address: Address,
    result: Result<bool, ForkReadError>,
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
  history: HashMap<HistoryQuery, Vec<Reply<HistoryResult>>>,
  balances: HashMap<AddressWithSpace, Vec<Reply<U256>>>,
  state_values: HashMap<StateKey, Vec<Reply<Option<StateValue>>>>,
  storage_points_initializations: HashMap<Address, Vec<Reply<bool>>>,
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

#[cfg(test)]
mod tests {
  use super::*;
  use crate::fork::tests::{pivot, rpc};
  use alloy_transport::mock::Asserter;
  use cfx_rpc_cfx_types::RpcAddress;
  use tokio::sync::oneshot;
  use xcfx_state::account::{Account, StoragePointsState};

  fn service(responses: &Asserter) -> ForkService {
    let (_, requests) = mpsc::channel(READ_QUEUE_CAPACITY);
    let (_, stopping) = watch::channel(false);
    ForkService {
      rpc: Arc::new(rpc(responses)),
      requests,
      stopping,
      cache: ForkCache::new(ForkCacheConfig::default()),
      pending: PendingReads::default(),
      reads: JoinSet::new(),
    }
  }

  fn read_flag(
    service: &mut ForkService,
    address: Address,
  ) -> oneshot::Receiver<Result<bool, ForkReadError>> {
    let (reply, response) = oneshot::channel();
    service.start_read(ReadRequest::StoragePointsInitialization { address, reply });
    response
  }

  fn absent_account(responses: &Asserter, service: &ForkService) {
    let header = pivot(&service.rpc.base);
    responses.push_success(&Some(header.clone()));
    responses.push_success(&Option::<RpcAddress>::None);
    responses.push_success(&Some(header));
  }

  async fn complete(service: &mut ForkService) {
    let completed = service.reads.join_next().await.unwrap().unwrap();
    service.complete_read(completed);
  }

  #[tokio::test]
  async fn initialization_requests_coalesce_and_cache_false() {
    let responses = Asserter::new();
    let mut service = service(&responses);
    let address = Address::from([0x80; 20]);
    absent_account(&responses, &service);
    // Register before yielding so the two calls overlap deterministically.
    let first = read_flag(&mut service, address);
    let second = read_flag(&mut service, address);
    assert_eq!(service.reads.len(), 1);
    complete(&mut service).await;
    assert!(!first.await.unwrap().unwrap());
    assert!(!second.await.unwrap().unwrap());
    assert!(!read_flag(&mut service, address).await.unwrap().unwrap());
    assert!(service.reads.is_empty());
    assert_eq!(service.pending.waiter_count, 0);
    assert!(responses.read_q().is_empty());
  }

  #[tokio::test]
  async fn initialization_errors_are_shared_but_not_cached() {
    let responses = Asserter::new();
    let mut service = service(&responses);
    let address = Address::from([0x80; 20]);
    responses.push_failure_msg("state temporarily unavailable");
    let first = read_flag(&mut service, address);
    let second = read_flag(&mut service, address);
    complete(&mut service).await;
    assert!(matches!(first.await.unwrap(), Err(ForkReadError::Rpc(_))));
    assert!(matches!(second.await.unwrap(), Err(ForkReadError::Rpc(_))));

    absent_account(&responses, &service);
    let retry = read_flag(&mut service, address);
    complete(&mut service).await;
    assert!(!retry.await.unwrap().unwrap());
    assert!(!read_flag(&mut service, address).await.unwrap().unwrap());
    assert!(service.reads.is_empty());
    assert!(responses.read_q().is_empty());
  }

  #[tokio::test]
  async fn known_cached_accounts_resolve_without_a_probe() {
    for initialized in [false, true] {
      let responses = Asserter::new();
      let mut service = service(&responses);
      let address = Address::from([0x80; 20]);
      let mut account = Account::new_empty(&address.with_native_space());
      account.sponsor_info.storage_points = if initialized {
        StoragePointsState::Initialized(Default::default())
      } else {
        StoragePointsState::Uninitialized
      };
      service.cache.insert_state_value(
        StateKey::Account(address.with_native_space()),
        Some(StateValue::Account(Arc::new(account))),
      );
      assert_eq!(
        read_flag(&mut service, address).await.unwrap().unwrap(),
        initialized
      );
      assert!(service.reads.is_empty());
      assert!(responses.read_q().is_empty());
    }
  }

  #[tokio::test]
  async fn stopped_service_rejects_initialization_reads() {
    let responses = Asserter::new();
    let (client, mut task) = ForkReadTask::spawn(
      rpc(&responses),
      &Handle::current(),
      ForkCacheConfig::default(),
    );
    task.close().await.unwrap();
    let result = tokio::task::spawn_blocking(move || {
      client.storage_points_initialized(&Address::from([0x80; 20]))
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(ForkReadError::Closed)));
  }
}
