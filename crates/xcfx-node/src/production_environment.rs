//! Owns deterministic, checkpointable time inputs for local block production.

use thiserror::Error;

/// Stable production settings used when constructing or resetting a Runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProductionDefaults {
  timestamp_increment: u64,
}

impl ProductionDefaults {
  pub(crate) const fn new(timestamp_increment: u64) -> Self {
    Self {
      timestamp_increment,
    }
  }

  pub(crate) const fn build_environment(&self, reset_base_timestamp: u64) -> ProductionEnvironment {
    ProductionEnvironment {
      logical_timestamp: reset_base_timestamp,
      next_block_timestamp: None,
    }
  }
}

/// Recoverable production time state owned by `RuntimeState`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ProductionEnvironment {
  logical_timestamp: u64,
  next_block_timestamp: Option<u64>,
}

/// Immutable time input prepared for one block candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PreparedProductionEnvironment {
  timestamp: u64,
}

impl PreparedProductionEnvironment {
  pub(crate) const fn timestamp(&self) -> u64 {
    self.timestamp
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

    Ok(PreparedProductionEnvironment { timestamp })
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
