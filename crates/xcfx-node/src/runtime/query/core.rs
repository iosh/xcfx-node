//! Core queries over a selected committed state or fixed remote history.

use std::sync::Arc;

use cfx_types::{Address, H256, U256};
use primitives::block::BlockHeight;

use super::{EpochSelector, NodeRuntime, StateQueryError, StateSelector, StateSource};

impl NodeRuntime {
  /// Resolves a Core epoch query, rejecting an explicit number beyond the chain head.
  /// This does not require an executable state at the resulting height.
  pub(crate) fn core_epoch_number(&self, selector: EpochSelector) -> Option<BlockHeight> {
    match selector {
      EpochSelector::Number(height) if height > self.runtime_state.history.optimistic_height() => {
        None
      }
      selector => Some(self.epoch_height(selector)),
    }
  }

  /// Reads the selected balance without reconstructing a remote execution account.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_balance_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_balance(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_balance_at(address, epoch_height)?),
    }
  }

  /// Reads only the selected committed nonce, independently of pool occupancy.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_nonce_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_nonce(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_nonce_at(address, epoch_height)?),
    }
  }

  /// Returns code bytes without supplying missing remote collateral-owner metadata.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_code_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<Arc<Vec<u8>>, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_code(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_code_at(address, epoch_height)?),
    }
  }

  /// Preserves the distinction between an absent storage entry and its stored value.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_storage_at(
    &self,
    address: Address,
    position: H256,
    selector: StateSelector,
  ) -> Result<Option<H256>, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_storage(address, position)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_storage_at(address, position, epoch_height)?),
    }
  }
}
