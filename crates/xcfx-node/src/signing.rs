use std::{
  collections::{BTreeMap, BTreeSet},
  sync::Arc,
};

use cfx_types::{AddressSpaceUtil, AddressWithSpace};
use cfxkey::KeyPair;
use primitives::{SignedTransaction, Transaction};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("address {address:?} is already associated with a different signing key")]
pub(crate) struct SigningKeyConflict {
  pub(crate) address: AddressWithSpace,
}

/// Stable, instance-owned signing keys.
///
/// The same key pair may be addressed in either Conflux space. Signing keys stay
/// outside Runtime checkpoints: restoring chain state must not copy, remove, or
/// replace private key material.
#[derive(Default)]
pub(crate) struct SigningKeys {
  keys: BTreeMap<AddressWithSpace, Arc<KeyPair>>,
}

impl SigningKeys {
  pub(crate) fn add(&mut self, key_pair: KeyPair) -> Result<(), SigningKeyConflict> {
    let addresses = [
      key_pair.address().with_native_space(),
      key_pair.evm_address().with_evm_space(),
    ];

    for address in addresses {
      if let Some(existing) = self.keys.get(&address)
        && existing.as_ref() != &key_pair
      {
        return Err(SigningKeyConflict { address });
      }
    }

    let key_pair = Arc::new(key_pair);
    for address in addresses {
      self
        .keys
        .entry(address)
        .or_insert_with(|| Arc::clone(&key_pair));
    }

    Ok(())
  }

  pub(crate) fn can_sign_for(&self, address: AddressWithSpace) -> bool {
    self.keys.contains_key(&address)
  }

  pub(crate) fn sign(
    &self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Option<SignedTransaction> {
    debug_assert_eq!(transaction.space(), sender.space);

    self
      .keys
      .get(&sender)
      .map(|key_pair| transaction.sign(key_pair.secret()))
  }
}

/// Ephemeral authorization for impersonated senders.
///
/// This set intentionally has no relationship to signing keys and is not part
/// of Runtime checkpoints. `reset` clears it; `revert` leaves it as-is.
#[derive(Default)]
pub(crate) struct ImpersonationState {
  authorized: BTreeSet<AddressWithSpace>,
}

impl ImpersonationState {
  pub(crate) fn authorize(&mut self, address: AddressWithSpace) -> bool {
    self.authorized.insert(address)
  }

  pub(crate) fn revoke(&mut self, address: AddressWithSpace) -> bool {
    self.authorized.remove(&address)
  }

  pub(crate) fn is_authorized(&self, address: AddressWithSpace) -> bool {
    self.authorized.contains(&address)
  }

  pub(crate) fn clear(&mut self) {
    self.authorized.clear();
  }
}
