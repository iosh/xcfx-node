//! Operation-specific input preparation over a private, fixed state view.

use std::sync::Arc;

use cfx_executor::machine::Machine;
use xcfx_state::State;

use cfx_parameters::{
  consensus::{ONE_CFX_IN_DRIP, TRANSACTION_DEFAULT_EPOCH_BOUND},
  internal_contract_addresses::SYSTEM_STORAGE_ADDRESS,
};
use cfx_rpc_eth_types::AccountStateOverrideMode;
use cfx_types::{AddressSpaceUtil, AddressWithSpace, H256, Space, U256};
use cfx_vm_types::Env;

use crate::{
  chain::EpochView,
  execution::genesis_hash_storage_entries,
  runtime_transaction::fake_sign_for_execution,
  state::{balance::mint_balance, state_version::StateVersion},
  transaction_request::{
    EthereumTransactionType, RequestSpace, TransactionRequest,
    build::{TransactionFees, TransactionFields, TransactionTemplate, TransactionType},
  },
};

use super::{Simulation, SimulationContext, VirtualExecutionError, VirtualExecutionOverrides};

/// Public simulation policy is resolved before selecting or opening state.
pub(crate) struct SimulationRequest {
  request: TransactionRequest,
  transaction_type: TransactionType,
  fees: TransactionFees,
}

impl SimulationRequest {
  pub(crate) fn new(request: TransactionRequest) -> Result<Self, VirtualExecutionError> {
    let transaction_type = simulation_type(&request);
    let fees = simulation_fees(&request, transaction_type)?;
    Ok(Self {
      request,
      transaction_type,
      fees,
    })
  }
}

#[derive(Clone, Copy)]
pub(super) struct SimulationOptions {
  has_sender: bool,
  has_gas: bool,
  has_price: bool,
}

impl SimulationOptions {
  pub(super) fn has_sender(self) -> bool {
    self.has_sender
  }
  pub(super) fn has_gas(self) -> bool {
    self.has_gas
  }
  pub(super) fn has_price(self) -> bool {
    self.has_price
  }
  pub(super) fn charge_gas(self) -> bool {
    self.has_sender && self.has_gas && self.has_price
  }
  pub(super) fn recheck_fee(self) -> bool {
    self.has_sender && self.has_price
  }
}

enum SimulationInput<'a, 'request> {
  Public(&'a SimulationRequest),
  Submission(&'a TransactionTemplate<'request>),
}

impl SimulationInput<'_, '_> {
  fn request(&self) -> &TransactionRequest {
    match self {
      Self::Public(prepared) => &prepared.request,
      Self::Submission(template) => template.request(),
    }
  }

  fn sender(&self) -> AddressWithSpace {
    match self {
      Self::Public(prepared) => prepared
        .request
        .from
        .unwrap_or_default()
        .with_space(prepared.request.space()),
      Self::Submission(template) => template.sender(),
    }
  }

  fn nonce(&self) -> Option<U256> {
    match self {
      Self::Public(prepared) => prepared.request.nonce,
      Self::Submission(template) => Some(template.nonce()),
    }
  }

  fn options(&self) -> SimulationOptions {
    let request = self.request();
    let (has_sender, has_price) = match self {
      Self::Public(_) => (
        request.from.is_some(),
        request.gas_price.is_some() || request.has_dynamic_fees(),
      ),
      Self::Submission(_) => (true, true),
    };
    SimulationOptions {
      has_sender,
      has_price,
      has_gas: request.gas.is_some(),
    }
  }
}

impl<'a> Simulation<'a> {
  pub(crate) fn new(
    machine: &'a Machine,
    view: &EpochView,
    request: SimulationRequest,
    overrides: VirtualExecutionOverrides,
  ) -> Result<Self, VirtualExecutionError> {
    Self::prepare(
      machine,
      view,
      &view.state().version,
      SimulationInput::Public(&request),
      overrides,
    )
  }

  /// Uses the sender, nonce, type and fees already chosen by node preparation.
  pub(crate) fn for_estimation(
    machine: &'a Machine,
    view: &EpochView,
    version: &Arc<StateVersion>,
    template: &TransactionTemplate<'_>,
  ) -> Result<Self, VirtualExecutionError> {
    Self::prepare(
      machine,
      view,
      version,
      SimulationInput::Submission(template),
      VirtualExecutionOverrides::default(),
    )
  }

  fn prepare(
    machine: &'a Machine,
    view: &EpochView,
    version: &Arc<StateVersion>,
    input: SimulationInput<'_, '_>,
    overrides: VirtualExecutionOverrides,
  ) -> Result<Self, VirtualExecutionError> {
    let request = input.request();
    let sender = input.sender();
    let options = input.options();
    let parent = view.execution_parent();
    let epoch_height = parent
      .height
      .checked_add(1)
      .ok_or(VirtualExecutionError::EpochHeightExhausted)?;
    let params = machine.params();
    // Raw pivot gas also covers epochs with no eSpace packing allowance.
    let default_gas = overrides.environment.gas_limit.unwrap_or(parent.gas_limit);
    let gas = request.gas.unwrap_or(default_gas);
    let storage_limit = request.storage_limit().unwrap_or(u64::MAX);
    let VirtualExecutionOverrides {
      state: state_overrides,
      environment,
    } = overrides;
    let state_overrides = state_overrides.into_state_overrides(sender.space)?;
    let mut state = if state_overrides.is_empty() {
      State::from_database(version.open_database())?
    } else {
      State::from_database_with_override(version.open_database(), &state_overrides, sender.space)?
    };

    // An explicit Core override takes precedence over the initial BLOCKHASH slots.
    let system_storage_override = state_overrides
      .get(&SYSTEM_STORAGE_ADDRESS)
      .filter(|_| sender.space == Space::Native)
      .map(|account| &account.state);
    for (slot, hash) in genesis_hash_storage_entries(machine, &parent, version) {
      let overridden = match system_storage_override {
        Some(AccountStateOverrideMode::State(_)) => true,
        Some(AccountStateOverrideMode::Diff(storage)) => storage.contains_key(&H256(slot)),
        _ => false,
      };
      if !overridden {
        state.set_system_storage(slot.into(), hash)?;
      }
    }
    if !options.has_sender {
      let increment = request
        .value
        .saturating_add(U256::from(1_000_000_000) * ONE_CFX_IN_DRIP)
        .min(U256::one() << 128);
      mint_balance(&mut state, &sender, increment)?;
    }
    let nonce = match input.nonce() {
      Some(nonce) => {
        state.set_nonce(&sender, &nonce)?;
        nonce
      }
      None => state.nonce(&sender)?,
    };
    let transaction = match &input {
      SimulationInput::Submission(template) => template.build(gas, storage_limit),
      SimulationInput::Public(prepared) => {
        let transaction_epoch = match request.space {
          RequestSpace::Native {
            epoch_height: requested,
            ..
          } => requested.unwrap_or(epoch_height),
          RequestSpace::Ethereum { .. } => epoch_height,
        };
        TransactionTemplate::new(
          request,
          prepared.transaction_type,
          TransactionFields {
            sender,
            nonce,
            chain_id: request
              .chain_id
              .unwrap_or_else(|| params.chain_id(epoch_height, sender.space)),
            epoch_height: transaction_epoch,
            fees: prepared.fees,
          },
        )
        .build(gas, storage_limit)
      }
    };
    // Identity is formed only after overrides and nonce resolution are complete.
    let transaction = fake_sign_for_execution(transaction, sender.address);
    let pos_env = view
      .pos_context()
      .env_input(&parent.pos_reference)
      .expect("a retained epoch's PoS context contains its parent reference");
    let base_gas_price = parent.base_price.unwrap_or_default();
    let mut burnt_gas_price = base_gas_price.map_all(|price| state.burnt_gas_price(price));
    if sender.space == Space::Ethereum {
      // Preserve effective fees and BASEFEE while allowing below-base-fee calls.
      burnt_gas_price[Space::Ethereum] =
        burnt_gas_price[Space::Ethereum].min(*transaction.gas_price());
    }
    let mut env = Env {
      chain_id: params.chain_id_map(epoch_height),
      number: view.next_epoch_start_block_number(),
      author: parent.author,
      timestamp: parent.timestamp,
      difficulty: U256::zero(),
      gas_limit: *transaction.gas(),
      last_hash: parent.hash,
      accumulated_gas_used: U256::zero(),
      epoch_height,
      pos_view: Some(pos_env.pos_view),
      finalized_epoch: Some(pos_env.finalized_epoch),
      transaction_epoch_bound: TRANSACTION_DEFAULT_EPOCH_BOUND,
      base_gas_price,
      burnt_gas_price,
      transaction_hash: transaction.hash(),
    };
    environment.apply(&mut env);
    let context = SimulationContext {
      state,
      spec: machine.spec(env.number, epoch_height),
      env,
      machine,
      options,
    };
    context.validate(&transaction)?;
    Ok(Self {
      context,
      transaction,
    })
  }
}

fn simulation_type(request: &TransactionRequest) -> TransactionType {
  if let RequestSpace::Native {
    transaction_type, ..
  } = request.space
  {
    return TransactionType::Native(transaction_type);
  }
  let transaction_type = if request.authorization_list().is_some() {
    EthereumTransactionType::Authorization
  } else if request.has_dynamic_fees() {
    EthereumTransactionType::DynamicFee
  } else if request.access_list.is_some() {
    EthereumTransactionType::AccessList
  } else {
    EthereumTransactionType::Legacy
  };
  TransactionType::Ethereum(transaction_type)
}

fn simulation_fees(
  request: &TransactionRequest,
  transaction_type: TransactionType,
) -> Result<TransactionFees, VirtualExecutionError> {
  if request.space() == Space::Native {
    return Ok(if transaction_type.has_dynamic_fee() {
      TransactionFees {
        gas_price: request
          .max_fee_per_gas
          .or(request.max_priority_fee_per_gas)
          .unwrap_or(U256::one()),
        priority_fee: request.max_priority_fee_per_gas.unwrap_or_default(),
      }
    } else {
      TransactionFees {
        gas_price: request.gas_price.unwrap_or(U256::one()),
        priority_fee: U256::zero(),
      }
    });
  }
  if !request.has_dynamic_fees() {
    let gas_price = request.gas_price.unwrap_or_default();
    return Ok(TransactionFees {
      gas_price,
      priority_fee: gas_price,
    });
  }
  let gas_price = request.max_fee_per_gas.unwrap_or_default();
  let priority_fee = request.max_priority_fee_per_gas.unwrap_or(gas_price);
  if priority_fee > gas_price {
    return Err(VirtualExecutionError::InvalidFees {
      max_fee: gas_price,
      priority_fee,
    });
  }
  Ok(TransactionFees {
    gas_price,
    priority_fee,
  })
}
