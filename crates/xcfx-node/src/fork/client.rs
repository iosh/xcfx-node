//! Synchronous access to the fork's remote state and history.

use std::sync::Arc;

use cfx_rpc_cfx_types::Block;
use cfx_types::{AddressWithSpace, H256, U256};
use primitives::{StorageKeyWithSpace, block::BlockHeight};
use tokio::sync::{mpsc, oneshot, watch};

use crate::execution::ExecutionCommitment;

use super::{
  ForkBase, ForkEpochReceipts, ForkReadError, HistoryQuery, HistoryResult, rpc::StateKey,
};

pub(super) type Reply<T> = oneshot::Sender<Result<T, ForkReadError>>;

/// Reads from one fixed remote base through a shared service.
///
/// Calls block the current thread. Use a blocking execution thread, such as
/// `spawn_blocking`, while the Tokio runtime serving requests remains active.
#[derive(Clone)]
pub(crate) struct ForkClient {
  base: Arc<ForkBase>,
  requests: mpsc::Sender<ReadRequest>,
  stopping: watch::Receiver<bool>,
}

impl ForkClient {
  pub(super) fn new(
    base: Arc<ForkBase>,
    requests: mpsc::Sender<ReadRequest>,
    stopping: watch::Receiver<bool>,
  ) -> Self {
    Self {
      base,
      requests,
      stopping,
    }
  }

  pub(crate) fn base(&self) -> &ForkBase {
    &self.base
  }

  pub(crate) fn balance(&self, address: &AddressWithSpace) -> Result<U256, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Balance {
      address: *address,
      reply,
    })?;
    self.receive(response)
  }

  pub(crate) fn state_value(
    &self,
    key: StorageKeyWithSpace<'_>,
  ) -> Result<Option<Box<[u8]>>, ForkReadError> {
    let key = StateKey::try_from(key)?;
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::StateValue { key, reply })?;
    let value = self.receive(response)?;
    Ok(value.map(|value| Box::<[u8]>::from(value.as_ref())))
  }

  pub(crate) fn pivot(&self, epoch: BlockHeight) -> Result<Arc<Block>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Pivot { epoch, reply })?;
    self.receive(response)
  }

  pub(crate) fn block(&self, epoch: BlockHeight, hash: H256) -> Result<Arc<Block>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Block { epoch, hash, reply })?;
    self.receive(response)
  }

  pub(crate) fn block_hashes(&self, epoch: BlockHeight) -> Result<Arc<[H256]>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::BlockHashes { epoch, reply })?;
    self.receive(response)
  }

  pub(crate) fn receipts(
    &self,
    epoch: BlockHeight,
    include_espace: bool,
  ) -> Result<Arc<ForkEpochReceipts>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Receipts {
      epoch,
      include_espace,
      reply,
    })?;
    self.receive(response)
  }

  pub(crate) fn commitment(
    &self,
    epoch: BlockHeight,
  ) -> Result<ExecutionCommitment, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Commitment { epoch, reply })?;
    self.receive(response)
  }

  /// Reads history only from the fixed remote prefix.
  ///
  /// # Errors
  ///
  /// Returns an error if required remote data is unavailable or inconsistent,
  /// or if the read service cannot complete the request.
  pub(crate) fn history(&self, query: HistoryQuery) -> Result<HistoryResult, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::History { query, reply })?;
    self.receive(response)
  }

  fn send(&self, request: ReadRequest) -> Result<(), ForkReadError> {
    if *self.stopping.borrow() {
      return Err(ForkReadError::Closed);
    }
    self
      .requests
      .blocking_send(request)
      .map_err(|_| self.disconnected_error())
  }

  fn receive<T>(
    &self,
    response: oneshot::Receiver<Result<T, ForkReadError>>,
  ) -> Result<T, ForkReadError> {
    response
      .blocking_recv()
      .map_err(|_| self.disconnected_error())?
  }

  fn disconnected_error(&self) -> ForkReadError {
    if *self.stopping.borrow() {
      ForkReadError::Closed
    } else {
      ForkReadError::ServiceStopped
    }
  }
}

pub(super) enum ReadRequest {
  History {
    query: HistoryQuery,
    reply: Reply<HistoryResult>,
  },
  Balance {
    address: AddressWithSpace,
    reply: Reply<U256>,
  },
  StateValue {
    key: StateKey,
    reply: Reply<Option<Arc<[u8]>>>,
  },
  Pivot {
    epoch: BlockHeight,
    reply: Reply<Arc<Block>>,
  },
  Block {
    epoch: BlockHeight,
    hash: H256,
    reply: Reply<Arc<Block>>,
  },
  BlockHashes {
    epoch: BlockHeight,
    reply: Reply<Arc<[H256]>>,
  },
  Receipts {
    epoch: BlockHeight,
    include_espace: bool,
    reply: Reply<Arc<ForkEpochReceipts>>,
  },
  Commitment {
    epoch: BlockHeight,
    reply: Reply<ExecutionCommitment>,
  },
}
