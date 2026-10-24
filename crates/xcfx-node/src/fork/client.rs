//! Synchronous access to the fork's remote state and history.

use std::sync::Arc;

use cfx_rpc_cfx_types::Block;
use cfx_rpc_eth_types::{Receipt as EthereumReceipt, Transaction as EthereumTransaction};
use cfx_types::{Address, AddressWithSpace, H256, Space, U256};
use primitives::{StorageKeyWithSpace, block::BlockHeight};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{execution::ExecutionCommitment, rpc_client::EthereumBlock};

use super::{
  EspaceStateQuery, ForkBase, ForkEpochReceipts, ForkReadError, HistoryBlockId, HistoryQuery,
  HistoryResult, rpc::StateKey,
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
  fn history(&self, query: HistoryQuery) -> Result<HistoryResult, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::History { query, reply })?;
    self.receive(response)
  }

  /// Reads a historical eSpace block checked against its Core pivot.
  pub(crate) fn espace_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<Arc<EthereumBlock>>, ForkReadError> {
    let HistoryResult::EthereumBlock(block) = self.history(HistoryQuery::Block {
      space: Space::Ethereum,
      selector,
      full_transactions,
    })?
    else {
      unreachable!("an eSpace block request must return an eSpace block");
    };
    Ok(block)
  }

  /// Reads a mined transaction from the fixed prefix, excluding remote pending entries.
  pub(crate) fn espace_transaction_by_hash(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<EthereumTransaction>>, ForkReadError> {
    let HistoryResult::EthereumTransaction(transaction) =
      self.history(HistoryQuery::Transaction {
        space: Space::Ethereum,
        hash,
      })?
    else {
      unreachable!("an eSpace transaction request must return an eSpace transaction");
    };
    Ok(transaction)
  }

  /// Resolves the block's hash list before reading and checking one transaction.
  pub(crate) fn espace_transaction_by_block_and_index(
    &self,
    selector: HistoryBlockId,
    index: usize,
  ) -> Result<Option<Arc<EthereumTransaction>>, ForkReadError> {
    let HistoryResult::EthereumTransaction(transaction) =
      self.history(HistoryQuery::EspaceTransactionByBlockAndIndex { selector, index })?
    else {
      unreachable!("an eSpace block transaction request must return an eSpace transaction");
    };
    Ok(transaction)
  }

  /// Reads a historical receipt with its block membership checked.
  pub(crate) fn espace_receipt(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<EthereumReceipt>>, ForkReadError> {
    let HistoryResult::EthereumReceipt(receipt) = self.history(HistoryQuery::Receipt {
      space: Space::Ethereum,
      hash,
    })?
    else {
      unreachable!("an eSpace receipt request must return an eSpace receipt");
    };
    Ok(receipt)
  }

  /// Reads all receipts checked against the selected block's transaction sequence.
  pub(crate) fn espace_block_receipts(
    &self,
    selector: HistoryBlockId,
  ) -> Result<Option<Arc<[EthereumReceipt]>>, ForkReadError> {
    let HistoryResult::EthereumBlockReceipts(receipts) =
      self.history(HistoryQuery::EspaceBlockReceipts { selector })?
    else {
      unreachable!("an eSpace block receipts request must return eSpace block receipts");
    };
    Ok(receipts)
  }

  pub(crate) fn history_epoch_by_hash(
    &self,
    hash: H256,
    require_pivot: bool,
  ) -> Result<Option<BlockHeight>, ForkReadError> {
    let HistoryResult::EpochHeight(height) = self.history(HistoryQuery::EpochByHash {
      hash,
      require_pivot,
    })?
    else {
      unreachable!("a historical epoch lookup must return an epoch height");
    };
    Ok(height)
  }

  pub(crate) fn espace_balance_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::EspaceQuantity(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Balance(address),
    })?
    else {
      unreachable!("a balance query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn espace_nonce_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::EspaceQuantity(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Nonce(address),
    })?
    else {
      unreachable!("a nonce query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn espace_code_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Arc<Vec<u8>>, ForkReadError> {
    let HistoryResult::EspaceCode(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Code(address),
    })?
    else {
      unreachable!("a code query must return bytes");
    };
    Ok(value)
  }

  pub(crate) fn espace_storage_at(
    &self,
    address: Address,
    position: H256,
    epoch_height: BlockHeight,
  ) -> Result<H256, ForkReadError> {
    let HistoryResult::EspaceStorage(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Storage { address, position },
    })?
    else {
      unreachable!("a storage query must return a storage word");
    };
    Ok(value)
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
