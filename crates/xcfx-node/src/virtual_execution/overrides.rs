//! Temporary state and block overrides for one simulation.

use std::collections::BTreeMap;

use cfx_rpc_eth_types::{AccountOverride, AccountStateOverrideMode, StateOverride};
use cfx_types::{Address, H256, Space, U64, U256, address_util::AddressUtil};
use cfx_vm_types::Env;
use primitives::BlockNumber;

use super::VirtualExecutionError;

/// Per-call storage visibility for an overridden account.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum VirtualStorageOverride {
  /// Replaces the visible storage; omitted slots read as zero.
  Replace(BTreeMap<H256, H256>),
  /// Overrides the listed slots and preserves all other visible storage.
  Diff(BTreeMap<H256, H256>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct VirtualAccountOverride {
  pub(crate) balance: Option<U256>,
  pub(crate) nonce: Option<u64>,
  pub(crate) code: Option<Vec<u8>>,
  pub(crate) storage: Option<VirtualStorageOverride>,
}

/// State overrides for the single Space selected by the transaction.
///
/// Keeps product input independent from the override representation accepted
/// by `xcfx_state::State`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct VirtualStateOverrides {
  accounts: BTreeMap<Address, VirtualAccountOverride>,
}

impl VirtualStateOverrides {
  pub(crate) fn new(accounts: BTreeMap<Address, VirtualAccountOverride>) -> Self {
    Self { accounts }
  }

  pub(super) fn into_state_overrides(
    self,
    space: Space,
  ) -> Result<StateOverride, VirtualExecutionError> {
    let mut overrides = StateOverride::with_capacity(self.accounts.len());

    for (address, account) in self.accounts {
      if space == Space::Native && !address.is_genesis_valid_address() {
        return Err(VirtualExecutionError::InvalidNativeOverrideAddress(address));
      }

      let state = match account.storage {
        None => AccountStateOverrideMode::None,
        Some(VirtualStorageOverride::Replace(storage)) => {
          AccountStateOverrideMode::State(storage.into_iter().collect())
        }
        Some(VirtualStorageOverride::Diff(storage)) => {
          AccountStateOverrideMode::Diff(storage.into_iter().collect())
        }
      };

      overrides.insert(
        address,
        AccountOverride {
          balance: account.balance,
          nonce: account.nonce.map(U64::from),
          code: account.code.map(Into::into),
          state,
          move_precompile_to: None,
        },
      );
    }

    Ok(overrides)
  }
}

/// Per-call block environment values; omitted fields keep their derived value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct VirtualEnvironmentOverrides {
  pub(crate) number: Option<BlockNumber>,
  pub(crate) timestamp: Option<u64>,
  pub(crate) author: Option<Address>,
  pub(crate) difficulty: Option<U256>,
  pub(crate) gas_limit: Option<U256>,
}

impl VirtualEnvironmentOverrides {
  pub(super) fn apply(self, env: &mut Env) {
    if let Some(number) = self.number {
      env.number = number;
    }
    if let Some(timestamp) = self.timestamp {
      env.timestamp = timestamp;
    }
    if let Some(author) = self.author {
      env.author = author;
    }
    if let Some(difficulty) = self.difficulty {
      env.difficulty = difficulty;
    }
    if let Some(gas_limit) = self.gas_limit {
      env.gas_limit = gas_limit;
    }
  }
}

/// Temporary state and environment changes visible to one execution.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct VirtualExecutionOverrides {
  pub(crate) state: VirtualStateOverrides,
  pub(crate) environment: VirtualEnvironmentOverrides,
}
