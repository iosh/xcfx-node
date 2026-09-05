//! Typed persistent state-control intents and private effective-state preparation.

use std::{collections::BTreeSet, sync::Arc};

use cfx_executor::state::State;
use cfx_statedb::StateDb;
use cfx_types::{AddressWithSpace, Space, U256, address_util::AddressUtil};
use thiserror::Error;

use crate::state::{
  layered_mpt_state::LayeredMptState,
  state_version::{CommittedStateVersion, StateCandidate, StateVersion},
};

/// A Runtime control intent, not a protocol transaction or committed state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StateControl {
  /// Same-Space balance movement; does not change supply counters.
  Transfer {
    from: AddressWithSpace,
    to: AddressWithSpace,
    amount: U256,
  },

  /// Explicit balance issuance.
  Mint { to: AddressWithSpace, amount: U256 },

  /// Explicit balance destruction.
  Burn {
    from: AddressWithSpace,
    amount: U256,
  },

  /// Sets a nonce within the account's `u64` range without decreasing it.
  SetNonce {
    address: AddressWithSpace,
    nonce: U256,
  },
}

impl StateControl {
  /// Checks constraints that do not require reading current state.
  fn validate_input(&self) -> Result<(), StateControlValidationError> {
    match self {
      Self::Transfer { from, to, .. } => {
        if from.space != to.space {
          return Err(StateControlValidationError::TransferSpaceMismatch {
            from: *from,
            to: *to,
          });
        }

        validate_address(*from)?;
        validate_address(*to)
      }
      Self::Mint { to, .. } => validate_address(*to),
      Self::Burn { from, .. } => validate_address(*from),
      Self::SetNonce { address, nonce } => {
        validate_address(*address)?;

        let maximum = U256::from(u64::MAX);

        if *nonce > maximum {
          return Err(StateControlValidationError::NonceOutOfRange {
            address: *address,
            requested: *nonce,
            maximum,
          });
        }

        Ok(())
      }
    }
  }

  /// Returns accounts whose nonce can be changed by this control.
  fn nonce_target(&self) -> Option<AddressWithSpace> {
    match self {
      Self::SetNonce { address, .. } => Some(*address),
      _ => None,
    }
  }
}

fn validate_address(address: AddressWithSpace) -> Result<(), StateControlValidationError> {
  if address.space == Space::Native && !address.address.is_genesis_valid_address() {
    return Err(StateControlValidationError::InvalidNativeAddress { address });
  }

  Ok(())
}

/// Input errors detectable before candidate preparation.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub(crate) enum StateControlValidationError {
  #[error("a transfer must use the same space: from={from:?}, to={to:?}")]
  TransferSpaceMismatch {
    from: AddressWithSpace,
    to: AddressWithSpace,
  },

  #[error("the Native address is not valid for state control: {address:?}")]
  InvalidNativeAddress { address: AddressWithSpace },

  #[error(
    "nonce exceeds the supported account range for {address:?}: requested={requested}, maximum={maximum}"
  )]
  NonceOutOfRange {
    address: AddressWithSpace,
    requested: U256,
    maximum: U256,
  },
}

/// Identifies a global supply counter maintained by Conflux state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupplyCounter {
  TotalIssued,
  TotalEvmToken,
}

/// Errors discovered while preparing controls against a private candidate.
#[derive(Debug, Error)]
pub(crate) enum StateControlPreparationError {
  #[error(transparent)]
  StateBackend(#[from] cfx_statedb::Error),

  #[error("insufficient balance for {address:?}: requested={requested}, available={available}")]
  InsufficientBalance {
    address: AddressWithSpace,
    requested: U256,
    available: U256,
  },

  #[error("balance overflow for {address:?}: current={current}, amount={amount}")]
  BalanceOverflow {
    address: AddressWithSpace,
    current: U256,
    amount: U256,
  },

  #[error("supply overflow for {counter:?}: current={current}, amount={amount}")]
  SupplyOverflow {
    counter: SupplyCounter,
    current: U256,
    amount: U256,
  },

  #[error("nonce cannot decrease for {address:?}: current={current}, requested={requested}")]
  NonceDecrease {
    address: AddressWithSpace,
    current: U256,
    requested: U256,
  },
}

/// One atomic set of state controls.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct StateControlBatch {
  controls: Vec<StateControl>,
}

impl StateControlBatch {
  pub(crate) fn new(controls: Vec<StateControl>) -> Result<Self, StateControlValidationError> {
    for control in &controls {
      control.validate_input()?;
    }

    Ok(Self { controls })
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.controls.is_empty()
  }

  pub(crate) fn len(&self) -> usize {
    self.controls.len()
  }

  pub(crate) fn nonce_targets(&self) -> BTreeSet<AddressWithSpace> {
    self
      .controls
      .iter()
      .filter_map(StateControl::nonce_target)
      .collect()
  }
}

/// Persistent, input-validated controls accepted by the Runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct StateOverlay {
  controls: Vec<StateControl>,
}

impl StateOverlay {
  pub(crate) fn empty() -> Self {
    Self::default()
  }

  pub(crate) fn is_empty(&self) -> bool {
    self.controls.is_empty()
  }

  pub(crate) fn len(&self) -> usize {
    self.controls.len()
  }

  /// Builds the next overlay. Publication happens outside this type.
  pub(crate) fn with_appended(&self, batch: &StateControlBatch) -> Self {
    let mut controls = self.controls.clone();
    controls.extend(batch.controls.iter().cloned());

    Self { controls }
  }

  /// Prepares an immutable state on a private candidate; publication remains
  /// the Runtime's responsibility.
  pub(crate) fn prepare_effective_state(
    &self,
    committed_base: &CommittedStateVersion,
  ) -> Result<Arc<StateVersion>, StateControlPreparationError> {
    if self.is_empty() {
      return Ok(Arc::clone(&committed_base.version));
    }

    let (backend, prepared_receiver) = LayeredMptState::new_for_preparation(StateCandidate::new(
      Arc::clone(&committed_base.version),
    ));

    let mut state = State::new(StateDb::new(Box::new(backend)))?;

    for control in &self.controls {
      apply_control(&mut state, control)?;
    }

    // The fork currently exposes non-committing StateDb root preparation
    // through this genesis-named wrapper; it does not bind an epoch identity.
    let root = state.compute_state_root_for_genesis(None)?;
    let version = prepared_receiver
      .prepared_state()
      .expect("state root preparation must hand off its immutable version");

    assert_eq!(
      version.root_with_aux_info(),
      root,
      "prepared state must match the computed root",
    );

    Ok(version)
  }
}

fn apply_control(
  state: &mut State,
  control: &StateControl,
) -> Result<(), StateControlPreparationError> {
  match control {
    StateControl::Transfer { from, to, amount } => {
      if amount.is_zero() {
        return Ok(());
      }

      let from_balance = state.balance(from)?;

      if from_balance < *amount {
        return Err(StateControlPreparationError::InsufficientBalance {
          address: *from,
          requested: *amount,
          available: from_balance,
        });
      }

      if from == to {
        return Ok(());
      }

      let to_balance = state.balance(to)?;

      if to_balance.checked_add(*amount).is_none() {
        return Err(StateControlPreparationError::BalanceOverflow {
          address: *to,
          current: to_balance,
          amount: *amount,
        });
      }

      state.transfer_balance(from, to, amount)?;
    }

    StateControl::Mint { to, amount } => {
      if amount.is_zero() {
        return Ok(());
      }

      let balance = state.balance(to)?;

      if balance.checked_add(*amount).is_none() {
        return Err(StateControlPreparationError::BalanceOverflow {
          address: *to,
          current: balance,
          amount: *amount,
        });
      }

      let total_issued = state.total_issued_tokens();

      if total_issued.checked_add(*amount).is_none() {
        return Err(StateControlPreparationError::SupplyOverflow {
          counter: SupplyCounter::TotalIssued,
          current: total_issued,
          amount: *amount,
        });
      }

      if to.space == Space::Ethereum {
        let total_evm_tokens = state.total_espace_tokens();

        if total_evm_tokens.checked_add(*amount).is_none() {
          return Err(StateControlPreparationError::SupplyOverflow {
            counter: SupplyCounter::TotalEvmToken,
            current: total_evm_tokens,
            amount: *amount,
          });
        }
      }

      state.add_balance(to, amount)?;
      state.add_total_issued(*amount);

      if to.space == Space::Ethereum {
        state.add_total_evm_tokens(*amount);
      }
    }

    StateControl::Burn { from, amount } => {
      if amount.is_zero() {
        return Ok(());
      }

      let balance = state.balance(from)?;

      if balance < *amount {
        return Err(StateControlPreparationError::InsufficientBalance {
          address: *from,
          requested: *amount,
          available: balance,
        });
      }

      let total_issued = state.total_issued_tokens();

      assert!(
        total_issued >= *amount,
        "total issued supply must cover every burn",
      );

      if from.space == Space::Ethereum {
        let total_evm_tokens = state.total_espace_tokens();

        assert!(
          total_evm_tokens >= *amount,
          "total eSpace supply must cover every eSpace burn",
        );
      }

      state.sub_balance(from, amount)?;
      state.sub_total_issued(*amount);

      if from.space == Space::Ethereum {
        state.sub_total_evm_tokens(*amount);
      }
    }

    StateControl::SetNonce { address, nonce } => {
      let current = state.nonce(address)?;

      if *nonce < current {
        return Err(StateControlPreparationError::NonceDecrease {
          address: *address,
          current,
          requested: *nonce,
        });
      }

      if *nonce == current {
        return Ok(());
      }

      state.set_nonce(address, nonce)?;
    }
  }

  Ok(())
}
