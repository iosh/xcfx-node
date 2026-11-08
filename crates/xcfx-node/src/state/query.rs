//! Native field reads that do not reconstruct remote execution records.

use std::sync::Arc;

use cfx_types::{Address, AddressSpaceUtil, BigEndianHash, H256, U256};
use thiserror::Error;
use xcfx_state::database::Database;

use crate::fork::ForkReadError;

use super::state_version::StateVersion;

/// Field reads preserve remote failures without encoding them as storage IO errors.
#[derive(Debug, Error)]
pub(crate) enum StateReadError {
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
}

impl StateVersion {
  /// Reads a committed balance without constructing an execution State.
  ///
  /// # Errors
  /// Propagates state decoding and required fixed-base reads.
  pub(crate) fn native_balance(&self, address: Address) -> Result<U256, StateReadError> {
    match self {
      Self::Mpt(version) => Ok(
        version
          .open_database()
          .get_account_balance(&address.with_native_space())?,
      ),
      Self::Fork(version) => version.native_balance(address),
    }
  }

  /// Reads the state's nonce without considering the transaction pool.
  ///
  /// # Errors
  /// Propagates state decoding and required fixed-base reads.
  pub(crate) fn native_nonce(&self, address: Address) -> Result<U256, StateReadError> {
    match self {
      Self::Mpt(version) => Ok(
        version
          .open_database()
          .get_account(&address.with_native_space())?
          .map_or(U256::zero(), |account| account.nonce),
      ),
      Self::Fork(version) => version.native_nonce(address),
    }
  }

  /// Reads code bytes without requiring remote collateral-owner metadata.
  ///
  /// # Errors
  /// Propagates state decoding, remote reads, and a remote code-hash mismatch.
  pub(crate) fn native_code(&self, address: Address) -> Result<Arc<Vec<u8>>, StateReadError> {
    match self {
      Self::Mpt(version) => {
        let database = version.open_database();
        let address = address.with_native_space();
        let Some(account) = database.get_account(&address)? else {
          return Ok(Arc::default());
        };
        Ok(
          database
            .get_code(&address, &account.code_hash)?
            .map(|code| code.code().clone())
            .unwrap_or_default(),
        )
      }
      Self::Fork(version) => version.native_code(address),
    }
  }

  /// Preserves storage-entry absence, including local deletion and range clears.
  ///
  /// # Errors
  /// Propagates state decoding and required fixed-base reads.
  pub(crate) fn native_storage(
    &self,
    address: Address,
    position: H256,
  ) -> Result<Option<H256>, StateReadError> {
    match self {
      Self::Mpt(version) => Ok(
        version
          .open_database()
          .get_storage_entry(&address.with_native_space(), position.as_bytes())?
          .map(|entry| H256::from_uint(&entry.value())),
      ),
      Self::Fork(version) => version.native_storage(address, position),
    }
  }
}
