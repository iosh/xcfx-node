//! Executes one fully resolved transaction against an isolated Runtime view.

use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::{
  executive::{
    ChargeCollateral, ExecutionOutcome, ExecutiveContext, TransactOptions, TransactSettings,
  },
  machine::Machine,
  state::State,
};
use cfx_parameters::consensus::TRANSACTION_DEFAULT_EPOCH_BOUND;
use cfx_rpc_eth_types::{
  AccountOverride as ExecutorAccountOverride, AccountStateOverrideMode,
  StateOverride as ExecutorStateOverride,
};
use cfx_types::{Address, AddressSpaceUtil, H256, Space, U64, U256, address_util::AddressUtil};
use cfx_vm_types::Env;
use primitives::{BlockNumber, SignedTransaction, Transaction, transaction::TransactionError};
use thiserror::Error;

use crate::{
  block_producer::RuntimeBlock, pos::CommittedPosState,
  runtime_transaction::fake_sign_for_execution, state::state_version::StateVersion,
  transaction_ingress::TransactionValidationContext,
};

/// A fully resolved transaction and the sender to use for this execution.
///
/// The transaction determines the Space. Callers must resolve omitted request
/// fields before constructing this value.
pub(crate) struct VirtualExecutionRequest {
  transaction: Transaction,
  sender: Address,
}

impl VirtualExecutionRequest {
  pub(crate) fn new(transaction: Transaction, sender: Address) -> Self {
    Self {
      transaction,
      sender,
    }
  }

  fn into_signed_transaction(self) -> Result<SignedTransaction, VirtualExecutionError> {
    let transaction_space = self.transaction.space();
    if transaction_space == Space::Native && !self.sender.is_genesis_valid_address() {
      return Err(VirtualExecutionError::InvalidNativeSender(self.sender));
    }

    Ok(fake_sign_for_execution(
      self.transaction,
      self.sender.with_space(transaction_space),
    ))
  }
}

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
/// This type keeps the product input independent from the RPC type currently
/// required by `cfx-executor`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct VirtualStateOverrides {
  accounts: BTreeMap<Address, VirtualAccountOverride>,
}

impl VirtualStateOverrides {
  pub(crate) fn new(accounts: BTreeMap<Address, VirtualAccountOverride>) -> Self {
    Self { accounts }
  }

  fn into_executor_overrides(
    self,
    space: Space,
  ) -> Result<ExecutorStateOverride, VirtualExecutionError> {
    let mut overrides = ExecutorStateOverride::with_capacity(self.accounts.len());

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
        ExecutorAccountOverride {
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
  fn apply(self, env: &mut Env) {
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

#[derive(Debug, Error)]
pub(crate) enum VirtualExecutionError {
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),

  #[error(transparent)]
  Transaction(#[from] TransactionError),

  #[error("the Native virtual execution sender has invalid type bits: {0:?}")]
  InvalidNativeSender(Address),

  #[error("the Native state override address has invalid type bits: {0:?}")]
  InvalidNativeOverrideAddress(Address),
}

/// Executes against a private state candidate and discards every state change.
pub(crate) fn execute_virtual_transaction(
  machine: &Machine,
  parent_block: &RuntimeBlock,
  parent_pos_state: &CommittedPosState,
  effective_state: &Arc<StateVersion>,
  block_number: BlockNumber,
  request: VirtualExecutionRequest,
  overrides: VirtualExecutionOverrides,
) -> Result<ExecutionOutcome, VirtualExecutionError> {
  let signed_transaction = request.into_signed_transaction()?;
  let transaction_space = signed_transaction.space();

  let epoch_height = parent_block
    .header()
    .height()
    .checked_add(1)
    .expect("a committed parent must permit a virtual execution height");
  let params = machine.params();
  let execution_block_number = overrides.environment.number.unwrap_or(block_number);
  let spec = machine.spec(execution_block_number, epoch_height);
  let validation = TransactionValidationContext::new(params, &spec, epoch_height);
  validation.validate_for_packing(&signed_transaction)?;

  let VirtualExecutionOverrides { state, environment } = overrides;
  let executor_overrides = state.into_executor_overrides(transaction_space)?;
  let (database, _state_receiver) = effective_state.open_database();
  let mut state = if executor_overrides.is_empty() {
    State::new(database)?
  } else {
    State::new_with_override(database, &executor_overrides, transaction_space)?
  };

  let pos_reference = parent_block
    .header()
    .pos_reference()
    .as_ref()
    .expect("a committed parent block must contain a PoS reference");
  let pos_env = parent_pos_state
    .env_input(pos_reference)
    .expect("committed PoS state must contain the parent block reference");
  let base_gas_price = parent_block.header().base_price().unwrap_or_default();
  let burnt_gas_price = base_gas_price.map_all(|price| state.burnt_gas_price(price));

  let mut env = Env {
    chain_id: params.chain_id_map(epoch_height),
    number: block_number,
    author: *parent_block.header().author(),
    timestamp: parent_block.header().timestamp(),
    difficulty: U256::zero(),
    gas_limit: *signed_transaction.gas(),
    last_hash: parent_block.hash(),
    accumulated_gas_used: U256::zero(),
    epoch_height,
    pos_view: Some(pos_env.pos_view),
    finalized_epoch: Some(pos_env.finalized_epoch),
    transaction_epoch_bound: TRANSACTION_DEFAULT_EPOCH_BOUND,
    base_gas_price,
    burnt_gas_price,
    transaction_hash: signed_transaction.hash(),
  };
  environment.apply(&mut env);

  // Overrides enter State through its transaction cache. Saving once makes
  // them the execution baseline and satisfies ExecutiveContext's empty-cache
  // invariant; the returned snapshot is unnecessary because this State dies
  // with the request.
  drop(state.save());

  let outcome = ExecutiveContext::new(&mut state, &env, machine, &spec).transact(
    &signed_transaction,
    TransactOptions {
      observer: (),
      settings: TransactSettings {
        // This path executes a fully resolved transaction, so normal charging
        // and fee checks apply. Common validation already checked the epoch.
        // Virtual calls allow code supplied by a sender account override.
        charge_collateral: ChargeCollateral::Normal,
        charge_gas: true,
        check_base_price: true,
        check_epoch_bound: false,
        forbid_eoa_with_code: false,
      },
    },
  )?;

  Ok(outcome)
}
