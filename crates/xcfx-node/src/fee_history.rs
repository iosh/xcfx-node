//! Fee-history samples and validated percentile requests shared by local and fork reads.

use std::{
  hash::{Hash, Hasher},
  sync::Arc,
};

use cfx_types::U256;
use thiserror::Error;

/// Finite, nondecreasing percentiles in the inclusive range from zero to one hundred.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RewardPercentiles(Arc<[f64]>);

#[derive(Debug, Error)]
#[error("reward percentiles must be finite, between 0 and 100, and in nondecreasing order")]
pub(crate) struct InvalidRewardPercentiles;

impl RewardPercentiles {
  pub(crate) fn new(mut values: Vec<f64>) -> Result<Self, InvalidRewardPercentiles> {
    if values
      .iter()
      .any(|value| !value.is_finite() || !(0.0..=100.0).contains(value))
      || values.windows(2).any(|pair| pair[0] > pair[1])
    {
      return Err(InvalidRewardPercentiles);
    }
    // Equal request keys must hash equally, including the two floating-point zeroes.
    for value in &mut values {
      if *value == 0.0 {
        *value = 0.0;
      }
    }
    Ok(Self(values.into()))
  }

  pub(crate) fn as_slice(&self) -> &[f64] {
    &self.0
  }
}

// Construction excludes NaN and canonicalizes signed zero.
impl Eq for RewardPercentiles {}

impl Hash for RewardPercentiles {
  fn hash<H: Hasher>(&self, state: &mut H) {
    self.0.len().hash(state);
    for value in self.0.iter() {
      value.to_bits().hash(state);
    }
  }
}

#[derive(Clone)]
pub(crate) struct FeeHistoryBlock {
  base_fee: U256,
  gas_used_ratio: f64,
  rewards: Vec<U256>,
}

impl FeeHistoryBlock {
  pub(crate) fn new(base_fee: U256, gas_used_ratio: f64, rewards: Vec<U256>) -> Self {
    Self {
      base_fee,
      gas_used_ratio,
      rewards,
    }
  }

  pub(crate) fn rewards(&self) -> &[U256] {
    &self.rewards
  }

  pub(crate) fn into_parts(self) -> (U256, f64, Vec<U256>) {
    (self.base_fee, self.gas_used_ratio, self.rewards)
  }
}

/// A nonempty, contiguous sequence of samples using one percentile request.
/// Positions are assigned by the range owner, rather than repeated in every row.
#[derive(Clone)]
pub(crate) struct FeeHistoryRange {
  oldest_height: u64,
  blocks: Vec<FeeHistoryBlock>,
}

impl FeeHistoryRange {
  pub(crate) fn new(oldest_height: u64, blocks: Vec<FeeHistoryBlock>) -> Self {
    assert!(
      !blocks.is_empty(),
      "a fee history range must contain at least one block"
    );
    Self {
      oldest_height,
      blocks,
    }
  }

  pub(crate) fn newest_height(&self) -> u64 {
    self
      .oldest_height
      .checked_add(self.blocks.len() as u64 - 1)
      .expect("a resolved fee history range must fit the epoch height range")
  }

  pub(crate) fn blocks(&self) -> &[FeeHistoryBlock] {
    &self.blocks
  }

  pub(crate) fn into_parts(self) -> (u64, Vec<FeeHistoryBlock>) {
    (self.oldest_height, self.blocks)
  }
}

/// A completed history has a price for the block after its last sample.
/// The absent range is the protocol's empty response.
#[derive(Default)]
pub(crate) struct FeeHistory {
  range: Option<(FeeHistoryRange, U256)>,
}

impl FeeHistory {
  pub(crate) fn new(range: FeeHistoryRange, next_base_fee: U256) -> Self {
    Self {
      range: Some((range, next_base_fee)),
    }
  }

  pub(crate) fn into_parts(self) -> Option<(FeeHistoryRange, U256)> {
    self.range
  }
}
