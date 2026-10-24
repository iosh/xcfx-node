//! eSpace history reads checked against the corresponding Core pivot.
//!
//! eSpace block numbers are epoch heights, and block hashes identify Core
//! pivots. The parent module owns the surrounding fork-base checks.

use std::sync::Arc;

use cfx_rpc_eth_types::{
  BlockId, Log as EthereumLog, Receipt as EthereumReceipt, Transaction as EthereumTransaction,
};
use cfx_types::{BigEndianHash, H256, U256};
use primitives::block::BlockHeight;

use crate::rpc_client::EthereumBlock;

use super::{EspaceStateQuery, ForkReadError, ForkRpc, HistoryBlockId, HistoryResult};

impl ForkRpc {
  pub(super) async fn read_history_epoch(
    &self,
    hash: H256,
    require_pivot: bool,
  ) -> Result<Option<BlockHeight>, ForkReadError> {
    let Some(metadata) = self.rpc.cfx_getBlockByHash(hash).await? else {
      return Ok(None);
    };
    if metadata.hash != hash {
      return Err(ForkReadError::Inconsistent("state block hash"));
    }
    let Some(epoch) = metadata.epoch_number else {
      return Ok(None);
    };
    if epoch > self.base.epoch_height.into() {
      return Ok(None);
    }
    let height = epoch.as_u64();
    let pivot = self.load_historical_pivot(height).await?;
    if require_pivot && pivot.hash != hash {
      return Ok(None);
    }
    if pivot.hash != hash {
      self.load_block_at_pivot(height, pivot.hash, hash).await?;
    }
    Ok(Some(height))
  }

  pub(super) async fn read_espace_state(
    &self,
    epoch_height: BlockHeight,
    query: EspaceStateQuery,
  ) -> Result<HistoryResult, ForkReadError> {
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let block = BlockId::Hash {
      hash: pivot.hash,
      require_canonical: Some(true),
    };
    Ok(match query {
      EspaceStateQuery::Balance(address) => {
        HistoryResult::EspaceQuantity(self.rpc.eth_getBalance(address, block).await?)
      }
      EspaceStateQuery::Nonce(address) => {
        HistoryResult::EspaceQuantity(self.rpc.eth_getTransactionCount(address, block).await?)
      }
      EspaceStateQuery::Code(address) => {
        HistoryResult::EspaceCode(Arc::new(self.rpc.eth_getCode(address, block).await?.0))
      }
      EspaceStateQuery::Storage { address, position } => HistoryResult::EspaceStorage(
        self
          .rpc
          .eth_getStorageAt(address, position.into_uint(), block)
          .await?,
      ),
    })
  }

  pub(super) async fn read_espace_block_receipts(
    &self,
    selector: HistoryBlockId,
  ) -> Result<Option<Vec<EthereumReceipt>>, ForkReadError> {
    let Some(block) = self.read_espace_block(selector, false).await? else {
      return Ok(None);
    };
    let receipts = self
      .rpc
      .eth_getBlockReceipts(BlockId::Hash {
        hash: block.header.hash,
        require_canonical: Some(true),
      })
      .await?
      .ok_or(ForkReadError::Unavailable("eSpace block receipts"))?;
    if receipts.len() != block.transaction_count() {
      return Err(ForkReadError::Inconsistent("eSpace block receipt count"));
    }
    for (index, (receipt, hash)) in receipts.iter().zip(block.transaction_hashes()).enumerate() {
      if receipt.transaction_hash != hash
        || receipt.transaction_index != U256::from(index)
        || receipt.block_hash != block.header.hash
        || receipt.block_number != block.header.number
      {
        return Err(ForkReadError::Inconsistent(
          "eSpace block receipt membership",
        ));
      }
      validate_receipt_logs(receipt)?;
    }
    Ok(Some(receipts))
  }

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
      return match selector {
        HistoryBlockId::Epoch(_) | HistoryBlockId::Number(_) => {
          Err(ForkReadError::Unavailable("eSpace historical block"))
        }
        HistoryBlockId::Hash(_) => Ok(None),
      };
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
    if let Some(transactions) = block.full_transactions() {
      if transactions.iter().enumerate().any(|(index, transaction)| {
        transaction.block_hash != Some(block.header.hash)
          || transaction.block_number != Some(block.header.number)
          || transaction.transaction_index != Some(U256::from(index))
      }) {
        return Err(ForkReadError::Inconsistent(
          "eSpace block transaction position",
        ));
      }
    } else if full_transactions {
      return Err(ForkReadError::Unavailable("full eSpace block transactions"));
    }
    Ok(Some(if full_transactions {
      block
    } else {
      block.into_hashes()
    }))
  }

  pub(super) async fn read_espace_transaction_by_block_and_index(
    &self,
    selector: HistoryBlockId,
    index: usize,
  ) -> Result<Option<EthereumTransaction>, ForkReadError> {
    let Some(block) = self.read_espace_block(selector, false).await? else {
      return Ok(None);
    };
    let Some(hash) = block.transaction_hashes().nth(index) else {
      return Ok(None);
    };
    let index = U256::from(index);
    let transaction = self
      .rpc
      .eth_getTransactionByBlockHashAndIndex(block.header.hash, index)
      .await?
      .ok_or(ForkReadError::Unavailable("eSpace block transaction"))?;
    if transaction.hash != hash
      || transaction.block_hash != Some(block.header.hash)
      || transaction.block_number != Some(block.header.number)
      || transaction.transaction_index != Some(index)
    {
      return Err(ForkReadError::Inconsistent(
        "eSpace block transaction identity",
      ));
    }
    Ok(Some(transaction))
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
    if height > self.base.epoch_height {
      return Ok(None);
    }
    let index = transaction
      .transaction_index
      .ok_or(ForkReadError::Inconsistent(
        "missing eSpace transaction index",
      ))?;
    let block = self
      .read_espace_block(HistoryBlockId::Hash(block_hash), false)
      .await?
      .ok_or(ForkReadError::Unavailable("eSpace transaction block"))?;
    if block.header.number.as_u64() != height {
      return Err(ForkReadError::Inconsistent(
        "eSpace transaction block number",
      ));
    }
    validate_transaction_membership(&block, index, hash)?;
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
    if receipt.block_number.as_u64() > self.base.epoch_height {
      return Ok(None);
    }
    let block = self
      .read_espace_block(HistoryBlockId::Hash(receipt.block_hash), false)
      .await?
      .ok_or(ForkReadError::Unavailable("eSpace receipt block"))?;
    if block.header.number != receipt.block_number {
      return Err(ForkReadError::Inconsistent("eSpace receipt block number"));
    }
    validate_transaction_membership(&block, receipt.transaction_index, hash)?;
    validate_receipt_logs(&receipt)?;
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

fn validate_transaction_membership(
  block: &EthereumBlock,
  index: U256,
  hash: H256,
) -> Result<(), ForkReadError> {
  if index >= U256::from(block.transaction_count()) {
    return Err(ForkReadError::Inconsistent("eSpace transaction index"));
  }
  let index = index.as_usize();
  if block.transaction_hashes().nth(index) != Some(hash) {
    return Err(ForkReadError::Inconsistent(
      "eSpace transaction block membership",
    ));
  }
  Ok(())
}

fn validate_receipt_logs(receipt: &EthereumReceipt) -> Result<(), ForkReadError> {
  if receipt.logs.iter().any(|log| {
    log.block_hash != receipt.block_hash
      || log.block_number != receipt.block_number
      || log.transaction_hash != receipt.transaction_hash
      || log.transaction_index != receipt.transaction_index
      || log.removed
  }) {
    return Err(ForkReadError::Inconsistent("eSpace receipt log position"));
  }
  Ok(())
}
