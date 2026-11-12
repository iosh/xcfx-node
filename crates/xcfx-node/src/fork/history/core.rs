//! Core history reads with block and transaction positions bound to a pivot.
//!
//! The parent module owns the surrounding fork-base checks. Each read here
//! resolves and validates the Core-specific position within that boundary.

use std::{collections::HashSet, sync::Arc};

use cfx_rpc_cfx_types::{Block, BlockTransactions, EpochNumber, Log, Receipt, Transaction};
use cfx_types::{H256, Space, U256};
use primitives::{TransactionStatus, block::BlockHeight};

use super::{ForkReadError, ForkRpc, HistoryBlockId};

// The Core RPC filter accepts at most 128 block hashes.
const MAX_CORE_LOG_BLOCK_HASHES: usize = 128;

impl ForkRpc {
  pub(super) async fn read_core_skipped_blocks(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Arc<[H256]>, ForkReadError> {
    let pivot = self.load_historical_pivot(epoch_height).await?;
    let ordered = self.load_block_hashes(epoch_height, pivot.hash).await?;
    let skipped = self
      .rpc
      .cfx_get_skipped_blocks_by_epoch(EpochNumber::Num(epoch_height.into()))
      .await?;
    let mut known: HashSet<_> = ordered.into_iter().collect();
    if skipped.iter().any(|hash| !known.insert(*hash)) {
      return Err(ForkReadError::Inconsistent("skipped epoch block set"));
    }
    Ok(skipped.into())
  }

  pub(super) async fn read_core_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<Block>, ForkReadError> {
    let Some(block) = self.core_history_block(selector).await? else {
      return Ok(None);
    };
    Ok(Some(project_block_transactions(block, full_transactions)?))
  }

  pub(super) async fn read_core_block_with_pivot(
    &self,
    hash: H256,
    pivot: H256,
    epoch_height: BlockHeight,
  ) -> Result<Block, ForkReadError> {
    if self.load_historical_pivot(epoch_height).await?.hash != pivot {
      return Err(ForkReadError::PivotAssumption);
    }
    let Some(metadata) = self.rpc.cfx_get_block_by_hash(hash).await? else {
      return Err(ForkReadError::PivotAssumption);
    };
    if metadata.hash != hash {
      return Err(ForkReadError::Inconsistent("historical block hash"));
    }
    if metadata.epoch_number != Some(epoch_height.into()) {
      return Err(ForkReadError::PivotAssumption);
    }
    let block = self.load_block_at_pivot(epoch_height, pivot, hash).await?;
    project_block_transactions(block, true)
  }

  pub(super) async fn read_core_transaction(
    &self,
    hash: H256,
  ) -> Result<Option<Transaction>, ForkReadError> {
    let Some(transaction) = self.rpc.cfx_get_transaction_by_hash(hash).await? else {
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
      .cfx_get_block_by_hash(block_hash)
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
    let Some(receipt) = self.rpc.cfx_get_transaction_receipt(hash).await? else {
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
      let mut previous: Option<(usize, U256)> = None;
      for log in self.rpc.cfx_get_logs(chunk.to_vec()).await? {
        if log.epoch_number != Some(epoch_height.into()) {
          return Err(ForkReadError::Inconsistent("Core log position"));
        }
        let block_index = log
          .block_hash
          .and_then(|hash| chunk.iter().position(|expected| *expected == hash))
          .ok_or(ForkReadError::Inconsistent("Core log position"))?;
        let log_index = log
          .log_index
          .ok_or(ForkReadError::Unavailable("Core log index"))?;
        // This request has no address or topic predicate. Every nonempty block
        // starts at zero, and its logs must be consecutive and in execution order.
        let expected_index = match previous {
          Some((block, index)) if block == block_index => index.checked_add(U256::one()),
          Some((block, _)) if block > block_index => None,
          _ => Some(U256::zero()),
        };
        if expected_index != Some(log_index) {
          return Err(ForkReadError::Inconsistent("Core log order"));
        }
        previous = Some((block_index, log_index));
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
        let Some(metadata) = self
          .rpc
          .cfx_get_block_by_block_number(number.into())
          .await?
        else {
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
        let Some(metadata) = self.rpc.cfx_get_block_by_hash(hash).await? else {
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

fn project_block_transactions(mut block: Block, full: bool) -> Result<Block, ForkReadError> {
  // Pivot-assumption reads return full transactions. Validate the response
  // before projecting it to hashes for metadata-only callers.
  let BlockTransactions::Full(transactions) = &block.transactions else {
    return Err(ForkReadError::Unavailable("full block transactions"));
  };
  if !full {
    block.transactions = BlockTransactions::Hashes(
      transactions
        .iter()
        .map(|transaction| transaction.hash)
        .collect(),
    );
  }
  Ok(block)
}
