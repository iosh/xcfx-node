//! Owns deterministic, checkpointable inputs for local block production.

use std::cmp::{max, min};

use cfx_executor::spec::CommonParams;
use cfx_parameters::{
  block::DEFAULT_TARGET_BLOCK_GAS_LIMIT, consensus_internal::ELASTICITY_MULTIPLIER,
  genesis::GENESIS_ACCOUNT_ADDRESS,
};
use cfx_types::{Address, U256};
use primitives::block::BlockHeight;
use thiserror::Error;

/// Stable production settings used when constructing or resetting a Runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProductionDefaults {
  timestamp_increment: u64,
  author: Address,
  block_gas_target: u64,
}

impl ProductionDefaults {
  pub(crate) const fn new(timestamp_increment: u64) -> Self {
    Self {
      timestamp_increment,
      author: GENESIS_ACCOUNT_ADDRESS,
      block_gas_target: DEFAULT_TARGET_BLOCK_GAS_LIMIT,
    }
  }

  pub(crate) const fn with_author(mut self, author: Address) -> Self {
    self.author = author;
    self
  }

  pub(crate) const fn with_block_gas_target(mut self, block_gas_target: u64) -> Self {
    self.block_gas_target = block_gas_target;
    self
  }

  pub(crate) const fn build_environment(&self, reset_base_timestamp: u64) -> ProductionEnvironment {
    ProductionEnvironment {
      logical_timestamp: reset_base_timestamp,
      next_block_timestamp: None,
      author: self.author,
      block_gas_target: self.block_gas_target,
    }
  }
}

/// Recoverable production state owned by `RuntimeState`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProductionEnvironment {
  logical_timestamp: u64,
  next_block_timestamp: Option<u64>,
  author: Address,
  block_gas_target: u64,
}

/// Immutable production input prepared for one block candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparedProductionEnvironment {
  timestamp: u64,
  author: Address,
  block_gas_limit: U256,
}

impl PreparedProductionEnvironment {
  pub(crate) const fn timestamp(&self) -> u64 {
    self.timestamp
  }

  pub(crate) const fn author(&self) -> Address {
    self.author
  }

  pub(crate) const fn block_gas_limit(&self) -> U256 {
    self.block_gas_limit
  }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ProductionTimeError {
  #[error("next block timestamp {proposed} is lower than parent timestamp {parent}")]
  TimestampBeforeParent { proposed: u64, parent: u64 },

  #[error("timestamp {current} cannot be increased by {increment} without overflowing")]
  TimestampOverflow { current: u64, increment: u64 },
}

impl ProductionEnvironment {
  pub(crate) fn set_author(&mut self, author: Address) -> Address {
    self.author = author;
    author
  }

  pub(crate) fn set_block_gas_target(&mut self, block_gas_target: u64) -> u64 {
    self.block_gas_target = block_gas_target;
    block_gas_target
  }

  pub(crate) fn increase_time(&mut self, increment: u64) -> Result<u64, ProductionTimeError> {
    let timestamp = self.logical_timestamp.checked_add(increment).ok_or(
      ProductionTimeError::TimestampOverflow {
        current: self.logical_timestamp,
        increment,
      },
    )?;

    self.logical_timestamp = timestamp;

    Ok(timestamp)
  }

  pub(crate) fn set_next_block_timestamp(
    &mut self,
    timestamp: u64,
    parent_timestamp: u64,
  ) -> Result<u64, ProductionTimeError> {
    if timestamp < parent_timestamp {
      return Err(ProductionTimeError::TimestampBeforeParent {
        proposed: timestamp,
        parent: parent_timestamp,
      });
    }

    self.next_block_timestamp = Some(timestamp);

    Ok(timestamp)
  }

  pub(crate) fn prepare_next_block(
    &self,
    defaults: &ProductionDefaults,
    parent_timestamp: u64,
    parent_gas_limit: U256,
    epoch_height: BlockHeight,
    params: &CommonParams,
  ) -> Result<PreparedProductionEnvironment, ProductionTimeError> {
    let timestamp = match self.next_block_timestamp {
      Some(timestamp) => timestamp,
      None => self
        .logical_timestamp
        .checked_add(defaults.timestamp_increment)
        .ok_or(ProductionTimeError::TimestampOverflow {
          current: self.logical_timestamp,
          increment: defaults.timestamp_increment,
        })?,
    };

    assert!(
      timestamp >= parent_timestamp,
      "prepared timestamp {timestamp} must not be lower than parent timestamp {parent_timestamp}",
    );

    let (gas_lower, gas_upper) = block_gas_limit_bounds(parent_gas_limit, epoch_height, params);

    let block_gas_target = U256::from(self.block_gas_target);

    let block_gas_target = if epoch_height >= params.transition_heights.cip1559 {
      block_gas_target * U256::from(ELASTICITY_MULTIPLIER as u64)
    } else {
      block_gas_target
    };

    let block_gas_limit = min(max(block_gas_target, gas_lower), gas_upper);

    Ok(PreparedProductionEnvironment {
      timestamp,
      author: self.author,
      block_gas_limit,
    })
  }

  pub(crate) fn commit_prepared(&mut self, prepared: PreparedProductionEnvironment) {
    self.commit_timestamp(prepared.timestamp);
  }

  /// Synchronizes a successfully committed block supplied outside production.
  pub(crate) fn synchronize_committed_timestamp(&mut self, committed_timestamp: u64) {
    self.commit_timestamp(committed_timestamp);
  }

  fn commit_timestamp(&mut self, committed_timestamp: u64) {
    self.logical_timestamp = committed_timestamp;
    self.next_block_timestamp = None;
  }
}

pub(crate) fn block_gas_limit_bounds(
  parent_gas_limit: U256,
  epoch_height: BlockHeight,
  params: &CommonParams,
) -> (U256, U256) {
  let elasticity_multiplier = U256::from(ELASTICITY_MULTIPLIER as u64);

  let parent_gas_limit = if epoch_height == params.transition_heights.cip1559 {
    parent_gas_limit
      .checked_mul(elasticity_multiplier)
      .expect("a parent gas limit adjusted at the CIP-1559 transition must fit U256")
  } else {
    parent_gas_limit
  };

  assert!(
    parent_gas_limit >= params.min_gas_limit,
    "parent gas limit {parent_gas_limit} must not be lower than minimum {}",
    params.min_gas_limit,
  );

  let divisor = params.gas_limit_bound_divisor;
  assert!(
    !divisor.is_zero(),
    "gas limit bound divisor must be non-zero"
  );

  let adjustment = parent_gas_limit / divisor;
  assert!(
    !adjustment.is_zero(),
    "parent gas limit {parent_gas_limit} must permit a non-zero bounded adjustment",
  );

  let gas_lower = max(
    parent_gas_limit
      .checked_sub(adjustment)
      .and_then(|limit| limit.checked_add(U256::one()))
      .expect("the lower block gas limit bound must fit U256"),
    params.min_gas_limit,
  );

  let gas_upper = parent_gas_limit
    .checked_add(adjustment)
    .and_then(|limit| limit.checked_sub(U256::one()))
    .expect("the upper block gas limit bound must fit U256");

  (gas_lower, gas_upper)
}
