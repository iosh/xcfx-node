//! Typed history reads constrained to the fork's fixed remote prefix.
//!
//! This module owns prefix checks, request dispatch, and shared result wrapping.
//! The `core` and `espace` modules resolve positions and validate responses.

mod core;
mod espace;

use std::sync::Arc;

use cfx_rpc_cfx_types::{Block, Log, Receipt, Transaction};
use cfx_rpc_eth_types::{
  Log as EthereumLog, Receipt as EthereumReceipt, Transaction as EthereumTransaction,
};
use cfx_types::{Address, H256, Space, U256};
use primitives::{BlockNumber, block::BlockHeight};

use crate::rpc_client::EthereumBlock;

use super::{ForkReadError, rpc::ForkRpc};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum HistoryBlockId {
  /// The pivot block of an epoch.
  Epoch(BlockHeight),
  /// A Core execution-order number, or an eSpace epoch number.
  Number(BlockNumber),
  Hash(H256),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum EspaceStateQuery {
  Balance(Address),
  Nonce(Address),
  Code(Address),
  Storage { address: Address, position: H256 },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum HistoryQuery {
  EpochByHash {
    hash: H256,
    require_pivot: bool,
  },
  EspaceState {
    epoch_height: BlockHeight,
    query: EspaceStateQuery,
  },
  EspaceBlockReceipts {
    selector: HistoryBlockId,
  },
  EspaceTransactionByBlockAndIndex {
    selector: HistoryBlockId,
    index: usize,
  },
  Block {
    space: Space,
    selector: HistoryBlockId,
    full_transactions: bool,
  },
  Transaction {
    space: Space,
    hash: H256,
  },
  Receipt {
    space: Space,
    hash: H256,
  },
  Logs {
    space: Space,
    epoch_height: BlockHeight,
  },
}

/// Results shared by callers waiting for the same history request.
#[derive(Clone)]
pub(super) enum HistoryResult {
  EpochHeight(Option<BlockHeight>),
  EspaceQuantity(U256),
  EspaceCode(Arc<Vec<u8>>),
  EspaceStorage(H256),
  EthereumBlockReceipts(Option<Arc<[EthereumReceipt]>>),
  CoreBlock(Option<Arc<Block>>),
  EthereumBlock(Option<Arc<EthereumBlock>>),
  CoreTransaction(Option<Arc<Transaction>>),
  EthereumTransaction(Option<Arc<EthereumTransaction>>),
  CoreReceipt(Option<Arc<Receipt>>),
  EthereumReceipt(Option<Arc<EthereumReceipt>>),
  CoreLogs(Arc<[Log]>),
  EthereumLogs(Arc<[EthereumLog]>),
}

impl ForkRpc {
  pub(super) async fn history(&self, query: HistoryQuery) -> Result<HistoryResult, ForkReadError> {
    // Numeric requests outside the prefix do not depend on remote availability.
    // Hash requests need a remote lookup before their position can be checked.
    match query {
      HistoryQuery::EspaceState { epoch_height, .. } if epoch_height > self.base.epoch_height => {
        return Err(ForkReadError::Unavailable(
          "historical state outside the fixed prefix",
        ));
      }
      HistoryQuery::EspaceBlockReceipts {
        selector: HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height),
      } if height > self.base.epoch_height => {
        return Ok(HistoryResult::EthereumBlockReceipts(None));
      }
      HistoryQuery::EspaceTransactionByBlockAndIndex {
        selector: HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height),
        ..
      } if height > self.base.epoch_height => {
        return Ok(HistoryResult::EthereumTransaction(None));
      }
      HistoryQuery::Block {
        space, selector, ..
      } => {
        let outside = match (space, selector) {
          (Space::Native, HistoryBlockId::Number(number)) => number > self.base.pivot_block_number,
          (_, HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height)) => {
            height > self.base.epoch_height
          }
          (_, HistoryBlockId::Hash(_)) => false,
        };
        if outside {
          return Ok(match space {
            Space::Native => HistoryResult::CoreBlock(None),
            Space::Ethereum => HistoryResult::EthereumBlock(None),
          });
        }
      }
      HistoryQuery::Logs {
        space,
        epoch_height,
      } if epoch_height > self.base.epoch_height => {
        return Ok(match space {
          Space::Native => HistoryResult::CoreLogs(Arc::from([])),
          Space::Ethereum => HistoryResult::EthereumLogs(Arc::from([])),
        });
      }
      _ => {}
    }

    self.validate_base().await?;
    let result = self.read_history(query).await?;
    // Recheck every successful remote read, including misses and excluded positions.
    self.validate_base().await?;
    Ok(result)
  }

  async fn read_history(&self, query: HistoryQuery) -> Result<HistoryResult, ForkReadError> {
    Ok(match query {
      HistoryQuery::EpochByHash {
        hash,
        require_pivot,
      } => HistoryResult::EpochHeight(self.read_history_epoch(hash, require_pivot).await?),
      HistoryQuery::EspaceState {
        epoch_height,
        query,
      } => self.read_espace_state(epoch_height, query).await?,
      HistoryQuery::EspaceBlockReceipts { selector } => HistoryResult::EthereumBlockReceipts(
        self
          .read_espace_block_receipts(selector)
          .await?
          .map(Arc::from),
      ),
      HistoryQuery::EspaceTransactionByBlockAndIndex { selector, index } => {
        HistoryResult::EthereumTransaction(
          self
            .read_espace_transaction_by_block_and_index(selector, index)
            .await?
            .map(Arc::new),
        )
      }
      HistoryQuery::Block {
        space: Space::Native,
        selector,
        full_transactions,
      } => {
        let block = self.read_core_block(selector, full_transactions).await?;
        HistoryResult::CoreBlock(block.map(Arc::new))
      }
      HistoryQuery::Block {
        space: Space::Ethereum,
        selector,
        full_transactions,
      } => {
        let block = self.read_espace_block(selector, full_transactions).await?;
        HistoryResult::EthereumBlock(block.map(Arc::new))
      }
      HistoryQuery::Transaction {
        space: Space::Native,
        hash,
      } => {
        let transaction = self.read_core_transaction(hash).await?;
        HistoryResult::CoreTransaction(transaction.map(Arc::new))
      }
      HistoryQuery::Transaction {
        space: Space::Ethereum,
        hash,
      } => {
        let transaction = self.read_espace_transaction(hash).await?;
        HistoryResult::EthereumTransaction(transaction.map(Arc::new))
      }
      HistoryQuery::Receipt {
        space: Space::Native,
        hash,
      } => {
        let receipt = self.read_core_receipt(hash).await?;
        HistoryResult::CoreReceipt(receipt.map(Arc::new))
      }
      HistoryQuery::Receipt {
        space: Space::Ethereum,
        hash,
      } => {
        let receipt = self.read_espace_receipt(hash).await?;
        HistoryResult::EthereumReceipt(receipt.map(Arc::new))
      }
      HistoryQuery::Logs {
        space: Space::Native,
        epoch_height,
      } => {
        let logs = self.read_core_logs(epoch_height).await?;
        HistoryResult::CoreLogs(logs.into())
      }
      HistoryQuery::Logs {
        space: Space::Ethereum,
        epoch_height,
      } => {
        let logs = self.read_espace_logs(epoch_height).await?;
        HistoryResult::EthereumLogs(logs.into())
      }
    })
  }
}
