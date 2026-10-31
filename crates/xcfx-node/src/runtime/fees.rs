//! Fee queries over one active history, with separate next-block preparation.

use std::{num::NonZeroU64, sync::Arc};

use cfx_parameters::block::espace_block_gas_limit_of_enabled_block;
use cfx_types::{Space, U256, U512};
use thiserror::Error;

use crate::{
  chain::EspaceBlockView,
  fee_history::{FeeHistory, FeeHistoryBlock, FeeHistoryRange, RewardPercentiles},
  fork::{ForkReadError, HistoryBlockId},
};

use super::{
  BlockPreparationError, EpochSelector, EspaceBlockSelector, EspaceBlockSource, NodeRuntime,
};

const MAX_BLOCK_COUNT: u64 = 1024;
const PRIORITY_FEE_SAMPLE_COUNT: u64 = 300;

#[derive(Debug, Error)]
pub(crate) enum FeeQueryError {
  #[error("fee history block is unknown or unavailable")]
  UnknownBlock,
  #[error(transparent)]
  Fork(#[from] ForkReadError),
  #[error(transparent)]
  Preparation(#[from] BlockPreparationError),
  #[error("suggested priority fee exceeds the U256 range")]
  SuggestionOverflow,
}

impl NodeRuntime {
  /// Reads a complete range and the price of its successor under one Runtime access.
  /// A missing successor at the head uses default block preparation without execution.
  ///
  /// # Errors
  /// Rejects unknown positions and propagates remote reads or next-block preparation
  /// failures. No partial history is returned when a required read fails.
  pub(crate) fn espace_fee_history(
    &self,
    block_count: u64,
    newest_block: EspaceBlockSelector,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<FeeHistory, FeeQueryError> {
    let Some(block_count) = NonZeroU64::new(block_count.min(MAX_BLOCK_COUNT)) else {
      return Ok(FeeHistory::default());
    };
    let newest_height = self.fee_history_end(newest_block)?;
    let range = self.read_fee_history_range(block_count, newest_height, reward_percentiles)?;
    let next_base_fee = self.fee_history_next_base_fee(range.newest_height())?;
    Ok(FeeHistory::new(range, next_base_fee))
  }

  /// Uses Conflux's fixed 300-epoch denominator even when history starts more recently.
  /// This statistic only reads historical rewards and never prepares a new block.
  pub(crate) fn espace_max_priority_fee_per_gas(&self) -> Result<U256, FeeQueryError> {
    let percentiles =
      RewardPercentiles::new(vec![50.0]).expect("a single median is a valid percentile request");
    let count =
      NonZeroU64::new(PRIORITY_FEE_SAMPLE_COUNT).expect("the priority fee sample count is nonzero");
    let newest_height = self.epoch_height(EpochSelector::LatestState);
    let history = self.read_fee_history_range(count, newest_height, &percentiles)?;
    let total = history.blocks().iter().fold(U512::zero(), |total, block| {
      total + U512::from(block.rewards()[0])
    });
    let ratio = self
      .chain_spec
      .machine()
      .params()
      .evm_transaction_block_ratio;
    let suggestion = total * U512::from(ratio) / U512::from(PRIORITY_FEE_SAMPLE_COUNT);
    U256::try_from(suggestion).map_err(|_| FeeQueryError::SuggestionOverflow)
  }

  fn fee_history_end(&self, selector: EspaceBlockSelector) -> Result<u64, FeeQueryError> {
    match self
      .espace_block_source(selector)
      .ok_or(FeeQueryError::UnknownBlock)?
    {
      EspaceBlockSource::Local(block) => Ok(block.epoch_height()),
      EspaceBlockSource::Fork { client, selector } => match selector {
        HistoryBlockId::Epoch(height) | HistoryBlockId::Number(height) => Ok(height),
        HistoryBlockId::Hash(hash) => client
          .history_epoch_by_hash(hash, true)?
          .ok_or(FeeQueryError::UnknownBlock),
      },
    }
  }

  fn read_fee_history_range(
    &self,
    block_count: NonZeroU64,
    newest_height: u64,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<FeeHistoryRange, FeeQueryError> {
    let mut oldest_height = newest_height.saturating_sub(block_count.get() - 1);
    let mut local_start_height = oldest_height;
    let mut blocks = Vec::new();
    if let Some(client) = self.reset_state.initial_view.fork_client() {
      let fork_height = client.base().epoch_height;
      if oldest_height <= fork_height {
        let remote_end_height = newest_height.min(fork_height);
        let remote_count = NonZeroU64::new(remote_end_height - oldest_height + 1)
          .expect("a selected remote fee history prefix is nonempty");
        let remote = Arc::unwrap_or_clone(client.espace_fee_history(
          remote_count,
          remote_end_height,
          reward_percentiles,
        )?);
        if newest_height <= fork_height {
          return Ok(remote);
        }
        (oldest_height, blocks) = remote.into_parts();
        // The crossing range has at least one local block above the fork height.
        local_start_height = fork_height + 1;
      }
    }
    for height in local_start_height..=newest_height {
      let block = self
        .runtime_state
        .history
        .epoch_at_height(height)
        .and_then(|view| view.espace_block())
        .expect("a resolved local fee history range must have contiguous epoch artifacts");
      blocks.push(block_fees(&block, reward_percentiles));
    }
    Ok(FeeHistoryRange::new(oldest_height, blocks))
  }

  fn fee_history_next_base_fee(&self, newest_height: u64) -> Result<U256, FeeQueryError> {
    let next_height = newest_height
      .checked_add(1)
      .ok_or(BlockPreparationError::PositionExhausted("epoch height"))?;
    if let Some(view) = self.epoch_at_height(next_height) {
      return Ok(
        view
          .execution_parent()
          .base_price
          .map_or(U256::zero(), |prices| prices[Space::Ethereum]),
      );
    }
    if let Some(client) = self.reset_state.initial_view.fork_client()
      && next_height <= client.base().epoch_height
    {
      let block = client
        .espace_block(HistoryBlockId::Epoch(next_height), false)?
        .expect("a fixed-prefix numeric block read must return a block or fail");
      return Ok(block.header.base_fee_per_gas.unwrap_or_default());
    }
    assert_eq!(
      newest_height,
      self.runtime_state.history.optimistic_height(),
      "only the current head may lack a fee history successor",
    );
    Ok(self.next_block_base_price()?[Space::Ethereum])
  }
}

fn block_fees(block: &EspaceBlockView, percentiles: &RewardPercentiles) -> FeeHistoryBlock {
  let percentiles = percentiles.as_slice();
  let header = block.pivot_header();
  let Some(base_fee) = header.space_base_price(Space::Ethereum) else {
    return FeeHistoryBlock::new(U256::zero(), 0.0, vec![U256::zero(); percentiles.len()]);
  };
  let gas_limit = espace_block_gas_limit_of_enabled_block(*header.gas_limit());
  let mut declared_gas = U512::zero();
  let mut prices = Vec::new();
  for view in block.transactions() {
    let transaction = view.transaction();
    let transaction = transaction.transaction();
    declared_gas += U512::from(*transaction.gas());
    if !percentiles.is_empty() {
      prices.push(if *transaction.gas_price() < base_fee {
        U256::zero()
      } else {
        transaction.effective_gas_price(&base_fee)
      });
    }
  }
  let gas_used_ratio = if gas_limit.is_zero() {
    0.0
  } else {
    integer_as_f64(declared_gas) / integer_as_f64(U512::from(gas_limit))
  };
  prices.sort_unstable();
  let rewards = percentiles
    .iter()
    .map(|percentile| {
      if prices.is_empty() {
        U256::zero()
      } else {
        let index = (*percentile * prices.len() as f64 / 100.0) as usize;
        prices[index.min(prices.len() - 1)]
      }
    })
    .collect();
  FeeHistoryBlock::new(base_fee, gas_used_ratio, rewards)
}

fn integer_as_f64(value: U512) -> f64 {
  if value.bits() <= 128 {
    return value.low_u128() as f64;
  }
  // Even the complete U512 range is finite in f64; wider sums need no truncation.
  value
    .to_big_endian()
    .iter()
    .fold(0.0, |value, byte| value * 256.0 + f64::from(*byte))
}
