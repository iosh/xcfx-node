//! Core fee-history rows stay within the prefix checked by the history service.

use std::num::NonZeroU64;

use cfx_rpc_cfx_types::EpochNumber;

use crate::fee_history::{FeeHistoryBlock, FeeHistoryRange, RewardPercentiles};

use super::{ForkReadError, ForkRpc};

impl ForkRpc {
  pub(super) async fn read_core_fee_history(
    &self,
    block_count: NonZeroU64,
    newest_height: u64,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<FeeHistoryRange, ForkReadError> {
    // Core accepts an epoch number, so the surrounding base checks bind this read.
    let response = self
      .rpc
      .cfx_fee_history(
        block_count.get().into(),
        EpochNumber::Num(newest_height.into()),
        reward_percentiles.as_slice(),
      )
      .await?;

    let oldest_height = response.oldest_epoch.as_u64();
    let requested_oldest = newest_height.saturating_sub(block_count.get() - 1);

    if oldest_height < requested_oldest || oldest_height > newest_height {
      return Err(ForkReadError::Inconsistent("Core fee history range"));
    }

    let count = usize::try_from(newest_height - oldest_height + 1)
      .expect("a bounded fee history range must fit the host index range");

    if response.base_fee_per_gas.len() != count + 1
      || response.gas_used_ratio.len() != count
      || response.reward.len() != count
      || response
        .gas_used_ratio
        .iter()
        .any(|ratio| !ratio.is_finite() || *ratio < 0.0)
      || response
        .reward
        .iter()
        .any(|rewards| rewards.len() != reward_percentiles.as_slice().len())
    {
      return Err(ForkReadError::Inconsistent("Core fee history columns"));
    }

    // A remote successor can lie beyond the fixed base. The Runtime supplies its own tail.
    let blocks = response
      .base_fee_per_gas
      .into_iter()
      .take(count)
      .zip(response.gas_used_ratio)
      .zip(response.reward)
      .map(|((base_fee, gas_used_ratio), rewards)| {
        FeeHistoryBlock::new(base_fee, gas_used_ratio, rewards)
      })
      .collect();

    Ok(FeeHistoryRange::new(oldest_height, blocks))
  }
}
