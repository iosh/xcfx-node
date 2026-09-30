//! Core history reads with block and transaction positions bound to a pivot.
//!
//! The parent module owns the surrounding fork-base checks. Each read here
//! resolves and validates the Core-specific position within that boundary.

use cfx_rpc_cfx_types::{Block, BlockTransactions, Log, Receipt, Transaction};
use cfx_types::{H256, Space};
use primitives::{TransactionStatus, block::BlockHeight};

use super::{ForkReadError, ForkRpc, HistoryBlockId};

// The Core RPC filter accepts at most 128 block hashes.
const MAX_CORE_LOG_BLOCK_HASHES: usize = 128;

impl ForkRpc {
  pub(super) async fn read_core_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<Block>, ForkReadError> {
    let Some(mut block) = self.core_history_block(selector).await? else {
      return Ok(None);
    };
    // Pivot-assumption reads return full transactions. Validate that response
    // before projecting it to hashes for callers that only need block metadata.
    let BlockTransactions::Full(transactions) = &block.transactions else {
      return Err(ForkReadError::Unavailable("full block transactions"));
    };
    if !full_transactions {
      block.transactions = BlockTransactions::Hashes(
        transactions
          .iter()
          .map(|transaction| transaction.hash)
          .collect(),
      );
    }
    Ok(Some(block))
  }

  pub(super) async fn read_core_transaction(
    &self,
    hash: H256,
  ) -> Result<Option<Transaction>, ForkReadError> {
    let Some(transaction) = self.rpc.cfx_getTransactionByHash(hash).await? else {
      return Ok(None);
    };
    if transaction.hash != hash {
      return Err(ForkReadError::Inconsistent("transaction hash"));
    }

    // The same hash can occur in multiple blocks, including skipped copies.
    // Preserve the RPC-selected position when reloading under a fixed pivot.
    let (block_hash, transaction_index) =
      match (transaction.block_hash, transaction.transaction_index) {
        (None, None) => return Ok(None),
        (Some(block_hash), Some(index)) => (block_hash, index),
        _ => {
          return Err(ForkReadError::Inconsistent(
            "incomplete transaction position",
          ));
        }
      };
    let metadata = self
      .rpc
      .cfx_getBlockByHash(block_hash)
      .await?
      .ok_or(ForkReadError::Unavailable("transaction block"))?;
    if metadata.hash != block_hash {
      return Err(ForkReadError::Inconsistent("transaction block hash"));
    }

    let Some(block) = self.load_core_block_from_metadata(metadata).await? else {
      return Ok(None);
    };
    let BlockTransactions::Full(transactions) = block.transactions else {
      return Err(ForkReadError::Unavailable("full block transactions"));
    };
    // Core RPC indexes include skipped Native transactions and exclude eSpace.
    let index = usize::try_from(transaction_index.as_u64())
      .map_err(|_| ForkReadError::OutOfRange("transaction index"))?;
    let transaction = transactions
      .into_iter()
      .nth(index)
      .ok_or(ForkReadError::Inconsistent("transaction index"))?;
    if transaction.hash != hash
      || transaction.block_hash != Some(block_hash)
      || transaction.transaction_index != Some(transaction_index)
    {
      return Err(ForkReadError::Inconsistent("transaction block membership"));
    }
    Ok(Some(transaction))
  }

  pub(super) async fn read_core_receipt(
    &self,
    hash: H256,
  ) -> Result<Option<Receipt>, ForkReadError> {
    let Some(receipt) = self.rpc.cfx_getTransactionReceipt(hash).await? else {
      return Ok(None);
    };
    if receipt.transaction_hash != hash {
      return Err(ForkReadError::Inconsistent("receipt transaction hash"));
    }
    let epoch = receipt
      .epoch_number
      .ok_or(ForkReadError::Unavailable("receipt epoch"))?;
    if epoch > self.base.epoch_height.into() {
      return Ok(None);
    }

    // Preserve both coordinates: a hash alone can identify a skipped duplicate.
    let block_hash = receipt.block_hash;
    let transaction_index = receipt.index;
    let epoch_height = epoch.as_u64();
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let receipts = self
      .load_receipts_at_pivot(epoch_height, pivot.hash, false)
      .await?;

    // Epoch receipts are grouped by block, including empty groups. Locate the
    // original block before applying its Native transaction index.
    let (_, block_receipts) = receipts
      .block_hashes
      .into_iter()
      .zip(receipts.receipts)
      .find(|(hash, _)| *hash == block_hash)
      .ok_or(ForkReadError::Inconsistent("receipt block membership"))?;
    let index = usize::try_from(transaction_index.as_u64())
      .map_err(|_| ForkReadError::OutOfRange("receipt transaction index"))?;
    let receipt = block_receipts
      .into_iter()
      .nth(index)
      .ok_or(ForkReadError::Inconsistent("receipt transaction index"))?;
    if receipt.transaction_hash != hash || receipt.index != transaction_index {
      return Err(ForkReadError::Inconsistent("receipt transaction identity"));
    }
    // A transaction receipt must describe execution, not a skipped occurrence.
    if receipt.outcome_status == TransactionStatus::Skipped.in_space(Space::Native).into() {
      return Err(ForkReadError::Inconsistent("skipped transaction receipt"));
    }
    Ok(Some(receipt))
  }

  pub(super) async fn read_core_logs(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Vec<Log>, ForkReadError> {
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let hashes = self.load_block_hashes(epoch_height, pivot.hash).await?;
    let mut logs = Vec::new();
    for chunk in hashes.chunks(MAX_CORE_LOG_BLOCK_HASHES) {
      for log in self.rpc.cfx_getLogs(chunk.to_vec()).await? {
        if log.epoch_number != Some(epoch_height.into())
          || !log.block_hash.is_some_and(|hash| chunk.contains(&hash))
        {
          return Err(ForkReadError::Inconsistent("Core log position"));
        }
        logs.push(log);
      }
    }
    Ok(logs)
  }

  async fn core_history_block(
    &self,
    selector: HistoryBlockId,
  ) -> Result<Option<Block>, ForkReadError> {
    match selector {
      HistoryBlockId::Epoch(epoch_height) => {
        let pivot = self.load_historical_pivot(epoch_height).await?;
        let block = self
          .load_block_at_pivot(epoch_height, pivot.hash, pivot.hash)
          .await?;
        if block.block_number != pivot.block_number {
          return Err(ForkReadError::Inconsistent("historical pivot block number"));
        }
        Ok(Some(block))
      }
      HistoryBlockId::Number(number) => {
        let Some(metadata) = self.rpc.cfx_getBlockByBlockNumber(number.into()).await? else {
          return Ok(None);
        };
        if metadata.block_number != Some(number.into()) {
          return Err(ForkReadError::Inconsistent("historical block number"));
        }
        if metadata.epoch_number.is_none() {
          return Err(ForkReadError::Unavailable("numbered block epoch"));
        }
        let block = self.load_core_block_from_metadata(metadata).await?;
        if block
          .as_ref()
          .is_some_and(|block| block.block_number != Some(number.into()))
        {
          return Err(ForkReadError::Inconsistent(
            "historical block number changed",
          ));
        }
        Ok(block)
      }
      HistoryBlockId::Hash(hash) => {
        let Some(metadata) = self.rpc.cfx_getBlockByHash(hash).await? else {
          return Ok(None);
        };
        if metadata.hash != hash {
          return Err(ForkReadError::Inconsistent("historical block hash"));
        }
        self.load_core_block_from_metadata(metadata).await
      }
    }
  }

  async fn load_core_block_from_metadata(
    &self,
    metadata: Block,
  ) -> Result<Option<Block>, ForkReadError> {
    let Some(epoch) = metadata.epoch_number else {
      return Ok(None);
    };
    if epoch > self.base.epoch_height.into() {
      return Ok(None);
    }

    // Metadata only locates the epoch. Reload with its pivot assumption so
    // execution-dependent fields belong to the selected historical ordering.
    let epoch_height = epoch.as_u64();
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let block = self
      .load_block_at_pivot(epoch_height, pivot.hash, metadata.hash)
      .await?;
    // A skipped block can belong to an epoch without an execution-order number.
    if block
      .block_number
      .is_some_and(|number| number > self.base.pivot_block_number.into())
    {
      return Err(ForkReadError::Inconsistent(
        "historical block number exceeds fork base",
      ));
    }
    Ok(Some(block))
  }
}
