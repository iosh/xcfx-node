//! eSpace history reads checked against the corresponding Core pivot.
//!
//! eSpace block numbers are epoch heights, and block hashes identify Core
//! pivots. The parent module owns the surrounding fork-base checks.

use cfx_rpc_eth_types::{
  Log as EthereumLog, Receipt as EthereumReceipt, Transaction as EthereumTransaction,
};
use cfx_types::H256;
use primitives::block::BlockHeight;

use crate::rpc_client::EthereumBlock;

use super::{ForkReadError, ForkRpc, HistoryBlockId};

impl ForkRpc {
  pub(super) async fn read_espace_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<EthereumBlock>, ForkReadError> {
    let block = match selector {
      HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height) => {
        self
          .rpc
          .eth_getBlockByNumber(height.into(), full_transactions)
          .await?
      }
      HistoryBlockId::Hash(hash) => self.rpc.eth_getBlockByHash(hash, full_transactions).await?,
    };
    let Some(block) = block else {
      return Ok(None);
    };
    let matches_selector = match selector {
      HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height) => {
        block.header.number.as_u64() == height
      }
      HistoryBlockId::Hash(hash) => block.header.hash == hash,
    };
    if !matches_selector {
      return Err(ForkReadError::Inconsistent("eSpace block identity"));
    }

    if !self
      .is_espace_history_block(block.header.number.as_u64(), block.header.hash)
      .await?
    {
      return Ok(None);
    }
    Ok(Some(block))
  }

  pub(super) async fn read_espace_transaction(
    &self,
    hash: H256,
  ) -> Result<Option<EthereumTransaction>, ForkReadError> {
    let Some(transaction) = self.rpc.eth_getTransactionByHash(hash).await? else {
      return Ok(None);
    };
    if transaction.hash != hash {
      return Err(ForkReadError::Inconsistent("eSpace transaction hash"));
    }
    // Pending transactions have no block position and are not remote history.
    let (height, block_hash) = match (transaction.block_number, transaction.block_hash) {
      (None, None) => return Ok(None),
      (Some(height), Some(block_hash)) => (height.as_u64(), block_hash),
      _ => {
        return Err(ForkReadError::Inconsistent(
          "incomplete eSpace transaction position",
        ));
      }
    };
    if !self.is_espace_history_block(height, block_hash).await? {
      return Ok(None);
    }
    Ok(Some(transaction))
  }

  pub(super) async fn read_espace_receipt(
    &self,
    hash: H256,
  ) -> Result<Option<EthereumReceipt>, ForkReadError> {
    let Some(receipt) = self.rpc.eth_getTransactionReceipt(hash).await? else {
      return Ok(None);
    };
    if receipt.transaction_hash != hash {
      return Err(ForkReadError::Inconsistent(
        "eSpace receipt transaction hash",
      ));
    }
    if !self
      .is_espace_history_block(receipt.block_number.as_u64(), receipt.block_hash)
      .await?
    {
      return Ok(None);
    }
    Ok(Some(receipt))
  }

  pub(super) async fn read_espace_logs(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Vec<EthereumLog>, ForkReadError> {
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let logs = self.rpc.eth_getLogs(pivot.hash).await?;
    if logs.iter().any(|log| {
      log.block_number.as_u64() != epoch_height || log.block_hash != pivot.hash || log.removed
    }) {
      return Err(ForkReadError::Inconsistent("eSpace log position"));
    }
    Ok(logs)
  }

  /// Positions beyond the fixed prefix are excluded; a pivot mismatch inside
  /// the prefix is an inconsistent response, not a missing history entry.
  async fn is_espace_history_block(
    &self,
    epoch_height: BlockHeight,
    hash: H256,
  ) -> Result<bool, ForkReadError> {
    if epoch_height > self.base.epoch_height {
      return Ok(false);
    }
    if self.load_historical_pivot(epoch_height).await?.hash != hash {
      return Err(ForkReadError::Inconsistent("eSpace historical pivot"));
    }
    Ok(true)
  }
}
