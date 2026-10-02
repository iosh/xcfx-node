//! Balance changes that keep the global issuance counters consistent.
//!
//! Mint and burn operate on a private state candidate. Callers must discard that
//! candidate on error; these helpers do not roll back changes.

use std::{collections::BTreeMap, fmt, sync::Arc};

use cfx_executor::state::State;
use cfx_statedb::StateDb;
use cfx_types::{AddressWithSpace, Space, U256};
use thiserror::Error;

use super::state_version::{StateCandidate, StateVersion};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SupplyCounter {
  TotalIssued,
  TotalEvmToken,
}

/// Display and Debug omit backend diagnostics; sources retain the original cause.
#[derive(Error)]
pub(crate) enum BalanceChangeError {
  #[error("balance state access failed")]
  State(#[from] cfx_statedb::Error),

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
}

impl fmt::Debug for BalanceChangeError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Issues a balance in a private candidate after checking both account and supply ranges.
/// The caller supplies an address already validated for its Space.
pub(crate) fn mint_balance(
  state: &mut State<'_>,
  to: &AddressWithSpace,
  amount: U256,
) -> Result<(), BalanceChangeError> {
  if amount.is_zero() {
    return Ok(());
  }

  let balance = state.balance(to)?;
  if balance.checked_add(amount).is_none() {
    return Err(BalanceChangeError::BalanceOverflow {
      address: *to,
      current: balance,
      amount,
    });
  }

  let total_issued = state.total_issued_tokens();
  if total_issued.checked_add(amount).is_none() {
    return Err(BalanceChangeError::SupplyOverflow {
      counter: SupplyCounter::TotalIssued,
      current: total_issued,
      amount,
    });
  }

  if to.space == Space::Ethereum {
    let total_evm_tokens = state.total_espace_tokens();
    if total_evm_tokens.checked_add(amount).is_none() {
      return Err(BalanceChangeError::SupplyOverflow {
        counter: SupplyCounter::TotalEvmToken,
        current: total_evm_tokens,
        amount,
      });
    }
  }

  state.add_balance(to, &amount)?;
  state.add_total_issued(amount);
  if to.space == Space::Ethereum {
    state.add_total_evm_tokens(amount);
  }
  Ok(())
}

/// Destroys a balance in a private candidate. Existing supply must cover that balance.
/// The caller supplies an address already validated for its Space.
pub(crate) fn burn_balance(
  state: &mut State<'_>,
  from: &AddressWithSpace,
  amount: U256,
) -> Result<(), BalanceChangeError> {
  if amount.is_zero() {
    return Ok(());
  }

  let balance = state.balance(from)?;
  if balance < amount {
    return Err(BalanceChangeError::InsufficientBalance {
      address: *from,
      requested: amount,
      available: balance,
    });
  }

  assert!(
    state.total_issued_tokens() >= amount,
    "total issued supply must cover every burn",
  );
  if from.space == Space::Ethereum {
    assert!(
      state.total_espace_tokens() >= amount,
      "total eSpace supply must cover every eSpace burn",
    );
  }

  state.sub_balance(from, &amount)?;
  state.sub_total_issued(amount);
  if from.space == Space::Ethereum {
    state.sub_total_evm_tokens(amount);
  }
  Ok(())
}

/// Sets startup balances without retaining control intents or changing other account fields.
/// Addresses are supplied by the signing-key registry; each address occurs once.
/// Failures discard the candidate and leave the parent version unchanged.
pub(crate) fn prepare_initial_balances(
  parent: Arc<StateVersion>,
  allocations: BTreeMap<AddressWithSpace, U256>,
) -> Result<Arc<StateVersion>, BalanceChangeError> {
  if allocations.is_empty() {
    return Ok(parent);
  }

  let mut candidate = StateCandidate::new(parent);
  let mut state = State::new(StateDb::new(&mut candidate))?;
  let mut balances = Vec::with_capacity(allocations.len());
  for (address, target) in allocations {
    balances.push((address, state.balance(&address)?, target));
  }

  // Burn first so a valid final supply cannot fail due to a temporary issuance peak.
  for (address, current, target) in &balances {
    if current > target {
      burn_balance(&mut state, address, *current - *target)?;
    }
  }
  for (address, current, target) in &balances {
    if target > current {
      mint_balance(&mut state, address, *target - *current)?;
    }
  }

  state.apply_changes_to_storage(None)?;
  drop(state);
  Ok(Arc::new(candidate.into_version()))
}
