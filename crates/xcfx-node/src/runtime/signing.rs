use alloy_dyn_abi::TypedData;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace};
use cfxkey::Signature;

use crate::signing::MessageSigningError;

use super::NodeRuntime;

impl NodeRuntime {
  /// Signs a personal message with an instance-owned key, without changing chain state.
  ///
  /// # Errors
  /// Returns an error when the requested account has no signing key.
  pub(crate) fn sign_message(
    &self,
    address: AddressWithSpace,
    message: &[u8],
  ) -> Result<Signature, MessageSigningError> {
    self.signing_keys.sign_message(address, message)
  }

  /// Signs EIP-712 data for an eSpace account using the supplied domain.
  ///
  /// # Errors
  /// Returns an error for invalid typed data or an account without a signing key.
  pub(crate) fn sign_typed_data(
    &self,
    address: Address,
    data: &TypedData,
  ) -> Result<Signature, MessageSigningError> {
    self
      .signing_keys
      .sign_typed_data(address.with_evm_space(), data)
  }
}
