use alloy_dyn_abi::{DynSolType, TypedData};
use cfx_types::{AddressWithSpace, H256};
use cfxkey::{KeyPair, Signature};
use thiserror::Error;

use super::SigningKeys;

#[derive(Debug, Error)]
pub(crate) enum MessageSigningError {
  #[error("no signing key for {address:?}")]
  KeyNotFound { address: AddressWithSpace },
  #[error("invalid typed data: {0}")]
  TypedData(#[from] alloy_dyn_abi::Error),
}

impl SigningKeys {
  /// Signs the EIP-191 digest of the decoded message bytes.
  pub(crate) fn sign_message(
    &self,
    address: AddressWithSpace,
    message: &[u8],
  ) -> Result<Signature, MessageSigningError> {
    let key = self
      .key_pair(address)
      .ok_or(MessageSigningError::KeyNotFound { address })?;
    let mut payload = b"\x19Ethereum Signed Message:\n".to_vec();
    payload.extend_from_slice(message.len().to_string().as_bytes());
    payload.extend_from_slice(message);
    Ok(sign_digest(key, keccak_hash::keccak(payload)))
  }

  /// Signs EIP-712 data using the domain supplied by the caller.
  /// The node's chain ID is neither inserted nor checked.
  /// A missing key is reported before typed-data validation.
  pub(crate) fn sign_typed_data(
    &self,
    address: AddressWithSpace,
    data: &TypedData,
  ) -> Result<Signature, MessageSigningError> {
    let key = self
      .key_pair(address)
      .ok_or(MessageSigningError::KeyNotFound { address })?;
    // The digest encoder assumes a struct; primitive primary types can panic.
    if data.primary_type != "EIP712Domain"
      && (!data.resolver.contains_type_name(&data.primary_type)
        || DynSolType::parse(&data.primary_type).is_ok())
    {
      return Err(alloy_dyn_abi::Error::custom("primaryType must be a struct").into());
    }
    Ok(sign_digest(key, H256::from(data.eip712_signing_hash()?.0)))
  }
}

fn sign_digest(key: &KeyPair, digest: H256) -> Signature {
  cfxkey::sign(key.secret(), &digest).expect("registered KeyPair contains a valid secp256k1 secret")
}
