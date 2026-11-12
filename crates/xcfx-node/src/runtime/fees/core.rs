//! Native fee history samples pivot blocks; the price oracle samples ordered blocks.

use std::{num::NonZeroU64, sync::Arc};

use cfx_parameters::{
  RATIO_BASE_TEN,
  block::CIP1559_CORE_TRANSACTION_GAS_RATIO,
  rpc::{GAS_PRICE_BLOCK_SAMPLE_SIZE, GAS_PRICE_DEFAULT_VALUE, GAS_PRICE_TRANSACTION_SAMPLE_SIZE},
};
use cfx_rpc_cfx_types::BlockTransactions;
use cfx_types::{Space, U256, U512};

use crate::{
  block_producer::RuntimeBlock,
  chain::EpochHistoryView,
  fee_history::{FeeHistory, FeeHistoryBlock, FeeHistoryRange, RewardPercentiles},
  fork::{ForkReadError, HistoryBlockId},
};

use super::{
  EpochSelector, FeeQueryError, MAX_BLOCK_COUNT, NodeRuntime, PRIORITY_FEE_SAMPLE_COUNT,
  fee_history_block,
};

impl NodeRuntime {
  pub(crate) fn core_fee_history(
    &self,
    block_count: u64,
    newest_epoch: EpochSelector,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<FeeHistory, FeeQueryError> {
    if matches!(newest_epoch, EpochSelector::LatestMined) {
      return Err(FeeQueryError::UnknownBlock);
    }

    let Some(count) = NonZeroU64::new(block_count.min(MAX_BLOCK_COUNT)) else {
      return Ok(FeeHistory::default());
    };

    let height = self
      .core_epoch_number(newest_epoch)
      .ok_or(FeeQueryError::UnknownBlock)?;

    let range = self.read_core_fee_history_range(count, height, reward_percentiles)?;
    let next_base_fee = self.fee_history_next_base_fee(range.newest_height(), Space::Native)?;

    Ok(FeeHistory::new(range, next_base_fee))
  }

  pub(crate) fn core_max_priority_fee_per_gas(&self) -> Result<U256, ForkReadError> {
    let percentiles =
      RewardPercentiles::new(vec![50.0]).expect("a median is a valid percentile request");
    let count = NonZeroU64::new(PRIORITY_FEE_SAMPLE_COUNT).expect("the sample count is nonzero");

    let height = self.epoch_height(EpochSelector::LatestState);
    let history = self.read_core_fee_history_range(count, height, &percentiles)?;
    let total = history.blocks().iter().fold(U512::zero(), |total, block| {
      total + U512::from(block.rewards()[0])
    });

    Ok(
      U256::try_from(total / U512::from(PRIORITY_FEE_SAMPLE_COUNT))
        .expect("an average of at most 300 U256 prices fits in U256"),
    )
  }

  fn read_core_fee_history_range(
    &self,
    count: NonZeroU64,
    newest_height: u64,
    percentiles: &RewardPercentiles,
  ) -> Result<FeeHistoryRange, ForkReadError> {
    let mut oldest_height = newest_height.saturating_sub(count.get() - 1);
    let mut local_start_height = oldest_height;
    let mut samples = Vec::new();

    if let Some(client) = self.reset_state.initial_view.fork_client() {
      let fork_height = client.base().epoch_height;

      if oldest_height <= fork_height {
        let remote_end = newest_height.min(fork_height);
        let remote_count = NonZeroU64::new(remote_end - oldest_height + 1)
          .expect("the selected remote prefix is nonempty");

        let remote =
          Arc::unwrap_or_clone(client.core_fee_history(remote_count, remote_end, percentiles)?);

        if newest_height <= fork_height {
          return Ok(remote);
        }

        (oldest_height, samples) = remote.into_parts();
        local_start_height = fork_height + 1;
      }
    }

    for height in local_start_height..=newest_height {
      let epoch = self
        .runtime_state
        .history
        .epoch_at_height(height)
        .expect("a resolved local fee history epoch must be retained");

      let pivot = epoch
        .artifacts()
        .expect("a local epoch has execution artifacts")
        .ordered_blocks()
        .last()
        .expect("an ordered epoch ends in its pivot");

      samples.push(pivot_fees(pivot.runtime_block(), percentiles));
    }

    Ok(FeeHistoryRange::new(oldest_height, samples))
  }

  /// Samples at most 100 ordered blocks and 10,000 Native transaction declarations.
  /// Genesis and skipped blocks do not contribute; repeated transaction copies do.
  pub(crate) fn core_gas_price(&self) -> Result<U256, ForkReadError> {
    let mut samples = GasPriceSamples::default();
    let mut height = self.runtime_state.history.optimistic_height();

    while height != 0 && !samples.full() {
      match self
        .epoch_history_at_height(height)
        .expect("an oracle epoch within the mined range has a source")
      {
        EpochHistoryView::Local(epoch) => {
          for block in epoch
            .artifacts()
            .expect("a local epoch has execution artifacts")
            .ordered_blocks()
            .iter()
            .rev()
          {
            let block = block.runtime_block();
            let transactions = block
              .transactions()
              .iter()
              .filter(|transaction| transaction.space() == Space::Native)
              .map(|transaction| (*transaction.gas(), *transaction.gas_price()));

            samples.push(*block.header().gas_limit(), transactions);

            if samples.full() {
              break;
            }
          }
        }

        EpochHistoryView::Fork {
          client,
          epoch_height,
        } => {
          let hashes = client.block_hashes(epoch_height)?;

          for hash in hashes.iter().rev() {
            let block = client
              .core_block(HistoryBlockId::Hash(*hash), true)?
              .ok_or(ForkReadError::Unavailable("gas-price sample block"))?;

            let gas_limit = remote_header_gas_limit(&block)?;
            let BlockTransactions::Full(transactions) = &block.transactions else {
              return Err(ForkReadError::Unavailable("gas-price sample transactions"));
            };

            samples.push(
              gas_limit,
              transactions
                .iter()
                .map(|transaction| (transaction.gas, transaction.gas_price)),
            );

            if samples.full() {
              break;
            }
          }
        }
      }

      height -= 1;
    }

    // The local pool has no independent minimum-price admission policy.
    Ok(samples.price())
  }
}

fn pivot_fees(block: &RuntimeBlock, percentiles: &RewardPercentiles) -> FeeHistoryBlock {
  let Some(base_fee) = block.header().space_base_price(Space::Native) else {
    return FeeHistoryBlock::new(
      U256::zero(),
      0.0,
      vec![U256::zero(); percentiles.as_slice().len()],
    );
  };

  let gas_limit = block.header().core_space_gas_limit();
  let mut declared_gas = U512::zero();
  let mut prices = Vec::new();

  for transaction in block.transactions() {
    if transaction.space() != Space::Native {
      continue;
    }

    let transaction = transaction.transaction();
    declared_gas += U512::from(*transaction.gas());

    if !percentiles.as_slice().is_empty() {
      prices.push(if *transaction.gas_price() < base_fee {
        U256::zero()
      } else {
        transaction.effective_gas_price(&base_fee)
      });
    }
  }

  fee_history_block(base_fee, gas_limit, declared_gas, prices, percentiles)
}

fn remote_header_gas_limit(block: &cfx_rpc_cfx_types::Block) -> Result<U256, ForkReadError> {
  let gas_limit = if block.base_fee_per_gas.is_some() {
    // Invert floor(9 * limit / 10) to its lower bound, as at fork initialization.
    // Core RPC alone leaves up to one gas of uncertainty in each remote header.
    let divisor = U512::from(CIP1559_CORE_TRANSACTION_GAS_RATIO);
    let numerator = U512::from(block.gas_limit) * U512::from(RATIO_BASE_TEN);

    U256::try_from((numerator + divisor - U512::one()) / divisor)
      .map_err(|_| ForkReadError::OutOfRange("Core gasLimit"))?
  } else {
    block.gas_limit
  };

  if gas_limit.is_zero() {
    return Err(ForkReadError::Inconsistent("zero sample block gas limit"));
  }

  Ok(gas_limit)
}

#[derive(Default)]
struct GasPriceSamples {
  blocks: usize,
  block_gas_limit: U512,
  transaction_gas_limit: U512,
  prices: Vec<U256>,
}

impl GasPriceSamples {
  fn full(&self) -> bool {
    self.blocks == GAS_PRICE_BLOCK_SAMPLE_SIZE
      || self.prices.len() == GAS_PRICE_TRANSACTION_SAMPLE_SIZE
  }

  fn push(&mut self, gas_limit: U256, transactions: impl Iterator<Item = (U256, U256)>) {
    self.blocks += 1;
    self.block_gas_limit += U512::from(gas_limit);

    let remaining = GAS_PRICE_TRANSACTION_SAMPLE_SIZE - self.prices.len();

    for (gas, price) in transactions.take(remaining) {
      self.transaction_gas_limit += U512::from(gas);
      self.prices.push(price);
    }
  }

  fn price(mut self) -> U256 {
    let default = U256::from(GAS_PRICE_DEFAULT_VALUE);

    if self.prices.is_empty() || self.transaction_gas_limit.is_zero() {
      return default;
    }

    let ratio = self.block_gas_limit / self.transaction_gas_limit;

    if ratio > U512::from(5) {
      return default;
    }

    self.prices.sort_unstable();

    let divisor = if ratio >= U512::from(2) { 8 } else { 2 };
    self.prices[self.prices.len() / divisor]
  }
}
