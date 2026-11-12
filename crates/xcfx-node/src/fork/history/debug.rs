//! Raw-block debug transactions are checked inside the fixed Core history prefix.

use cfx_rpc_cfx_types::BlockTransactions;
use cfx_rpc_eth_types::WrapTransaction;
use cfx_types::{H256, Space, U256};
use primitives::TransactionWithSignature;

use super::{ForkReadError, ForkRpc};

impl ForkRpc {
  pub(super) async fn read_core_debug_epoch(
    &self,
    height: u64,
  ) -> Result<Vec<WrapTransaction>, ForkReadError> {
    let pivot = self.load_historical_pivot(height).await?;
    let hashes = self.load_block_hashes(height, pivot.hash).await?;
    let mut transactions = Vec::new();

    for hash in hashes {
      transactions.extend(
        self
          .read_debug_transactions_at_pivot(hash, height, pivot.hash)
          .await?,
      );
    }

    Ok(transactions)
  }

  pub(super) async fn read_core_debug_block(
    &self,
    hash: H256,
  ) -> Result<Option<Vec<WrapTransaction>>, ForkReadError> {
    let Some(block) = self.rpc.cfx_get_block_by_hash(hash).await? else {
      return Ok(None);
    };

    if block.hash != hash {
      return Err(ForkReadError::Inconsistent("debug block hash"));
    }

    let Some(epoch) = block.epoch_number else {
      return Ok(None);
    };

    if epoch > U256::from(self.base.epoch_height) {
      return Ok(None);
    }

    let height = epoch.as_u64();
    let pivot = self.load_historical_pivot(height).await?;
    let hashes = self.load_block_hashes(height, pivot.hash).await?;

    if !hashes.contains(&hash) {
      return Ok(Some(Vec::new()));
    }

    Ok(Some(
      self
        .read_debug_transactions_at_pivot(hash, height, pivot.hash)
        .await?,
    ))
  }

  async fn read_debug_transactions_at_pivot(
    &self,
    hash: H256,
    height: u64,
    pivot: H256,
  ) -> Result<Vec<WrapTransaction>, ForkReadError> {
    let block = self.load_block_at_pivot(height, pivot, hash).await?;
    let BlockTransactions::Full(native) = block.transactions else {
      return Err(ForkReadError::Unavailable(
        "debug block Native transactions",
      ));
    };

    let transactions = self.rpc.debug_get_transactions_by_block(hash).await?;
    let mut native = native.iter();
    let mut espace_index = U256::zero();

    for transaction in &transactions {
      match transaction {
        WrapTransaction::NativeTransaction(transaction) => {
          if native.next() != Some(transaction) {
            return Err(ForkReadError::Inconsistent(
              "debug Native transaction membership",
            ));
          }
        }

        WrapTransaction::EthTransaction(transaction) => {
          if transaction.block_hash != Some(pivot)
            || transaction.block_number != Some(height.into())
            || transaction.transaction_index != Some(espace_index)
          {
            return Err(ForkReadError::Inconsistent(
              "debug eSpace transaction position",
            ));
          }

          if let Some(raw) = &transaction.raw {
            let decoded = TransactionWithSignature::from_raw(&raw.0)
              .map_err(|_| ForkReadError::Inconsistent("debug eSpace transaction encoding"))?;

            if decoded.hash() != transaction.hash || decoded.space() != Space::Ethereum {
              return Err(ForkReadError::Inconsistent(
                "debug eSpace transaction identity",
              ));
            }
          }

          espace_index += U256::one();
        }
      }
    }

    if native.next().is_some() {
      return Err(ForkReadError::Inconsistent(
        "incomplete debug Native transactions",
      ));
    }

    Ok(transactions)
  }
}
