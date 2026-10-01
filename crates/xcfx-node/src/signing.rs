mod accounts;

pub(crate) use accounts::{
  AccountConfigError, AccountSource, DerivationPathPrefix, MnemonicAccountConfig, MnemonicPhrase,
};

use std::collections::{BTreeMap, BTreeSet};

use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, Space};
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
  keys: Vec<KeyPair>,
  by_address: BTreeMap<AddressWithSpace, usize>,
}

impl SigningKeys {
  fn len(&self) -> usize {
    self.keys.len()
  }

  pub(crate) fn add(&mut self, key_pair: KeyPair) -> Result<(), SigningKeyConflict> {
    let addresses = [
      key_pair.address().with_native_space(),
      key_pair.evm_address().with_evm_space(),
    ];

    for address in addresses {
      if let Some(index) = self.by_address.get(&address)
        && self.keys[*index] != key_pair
      {
        return Err(SigningKeyConflict { address });
      }
    }

    // Both addresses are installed together; a duplicate keeps its original position.
    if self.by_address.contains_key(&addresses[0]) {
      return Ok(());
    }

    let index = self.keys.len();
    self.keys.push(key_pair);
    for address in addresses {
      self.by_address.insert(address, index);
    }

    Ok(())
  }

  pub(crate) fn can_sign_for(&self, address: AddressWithSpace) -> bool {
    self.by_address.contains_key(&address)
  }

  /// Returns addresses in the order their keys were first registered.
  pub(crate) fn addresses(&self, space: Space) -> Vec<Address> {
    self
      .keys
      .iter()
      .map(|key| match space {
        Space::Native => key.address(),
        Space::Ethereum => key.evm_address(),
      })
      .collect()
  }

  pub(crate) fn sign(
    &self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Option<SignedTransaction> {
    debug_assert_eq!(transaction.space(), sender.space);

    self
      .by_address
      .get(&sender)
      .map(|index| transaction.sign(self.keys[*index].secret()))
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
