//! Isolated call, gas estimation, and access-list execution over one fixed view.

mod access_list;
mod estimation;
mod observer;
mod overrides;
mod prepare;
mod validation;

pub(crate) use overrides::{
  VirtualAccountOverride, VirtualEnvironmentOverrides, VirtualExecutionOverrides,
  VirtualStateOverrides, VirtualStorageOverride,
};
pub(crate) use prepare::SimulationRequest;

use cfx_executor::{
  executive::{
    ChargeCollateral, ExecutionError, ExecutionOutcome, ExecutiveContext, ToRepackError,
    TransactOptions, TransactSettings, TxDropError,
  },
  machine::Machine,
};
use xcfx_state::State;

use cfx_types::{Address, U256, U512};
use cfx_vm_types::{Env, Spec};
use primitives::{AccessList, SignedTransaction, transaction::TransactionError};
use thiserror::Error;

use crate::state::balance::BalanceChangeError;

use prepare::SimulationOptions;

use observer::{AccessListObserver, ExecutionObserver, ObservedExecution};

#[derive(Debug, Error)]
pub(crate) enum SimulationFailure {
  #[error("transaction was not executed: {0:?}")]
  Drop(TxDropError),
  #[error("transaction was not executed: {0:?}")]
  Repack(ToRepackError),
  #[error("execution failed: {error:?}")]
  Execution {
    error: ExecutionError,
    output: Vec<u8>,
    // Present for Core, whose estimate RPC formats the observed unwind chain.
    errors: Option<Vec<(Address, String)>>,
  },
}

#[derive(Debug, Error)]
pub(crate) enum VirtualExecutionError {
  #[error("invalid Core address type bits in state override: {0:?}")]
  InvalidNativeOverrideAddress(Address),
  #[error("maxPriorityFeePerGas {priority_fee} exceeds maxFeePerGas {max_fee}")]
  InvalidFees { max_fee: U256, priority_fee: U256 },
  #[error(transparent)]
  Transaction(#[from] TransactionError),
  #[error("simulation state access failed")]
  State(#[from] cfx_statedb::Error),
  #[error(transparent)]
  Balance(#[from] BalanceChangeError),
  #[error(transparent)]
  Execution(#[from] SimulationFailure),
  #[error("the parent epoch height cannot advance for simulation")]
  EpochHeightExhausted,
  #[error("required storage limit cannot be represented: {required}")]
  StorageLimitOutOfRange { required: U512 },
}

pub(crate) struct TransactionEstimate {
  pub(crate) gas_limit: U256,
  pub(crate) storage_limit: u64,
}

pub(crate) struct AccessListResult {
  pub(crate) access_list: AccessList,
  pub(crate) gas_used: U256,
  pub(crate) error: Option<SimulationFailure>,
}

impl ObservedExecution {
  fn fail(self, error: ExecutionError) -> EstimatedOutcome {
    EstimatedOutcome::Failed {
      failure: SimulationFailure::Execution {
        error,
        output: self.executed.output,
        errors: self.errors,
      },
      gas_used: self.executed.gas_used,
      access_list: self.access_list,
    }
  }
}

enum ObservedOutcome {
  NotExecuted(SimulationFailure),
  Failed(ExecutionError, ObservedExecution),
  Finished(ObservedExecution),
}

struct EstimatedExecution {
  estimate: TransactionEstimate,
  output: Vec<u8>,
  protocol_gas_used: U256,
  access_list: Option<AccessList>,
}

enum EstimatedOutcome {
  NotExecuted(SimulationFailure),
  Failed {
    failure: SimulationFailure,
    gas_used: U256,
    access_list: Option<AccessList>,
  },
  Finished(EstimatedExecution),
}

/// Owns one private state for the entire operation; no outcome publishes it.
/// Multi-pass execution restores this state's prepared baseline between rounds.
pub(crate) struct Simulation<'a> {
  context: SimulationContext<'a>,
  transaction: SignedTransaction,
}

struct SimulationContext<'a> {
  state: State<'static>,
  env: Env,
  spec: Spec,
  machine: &'a Machine,
  options: SimulationOptions,
}

impl Simulation<'_> {
  pub(crate) fn call(mut self) -> Result<Vec<u8>, VirtualExecutionError> {
    match self.context.estimate(&self.transaction, false)? {
      EstimatedOutcome::Finished(result) => Ok(result.output),
      EstimatedOutcome::NotExecuted(failure) | EstimatedOutcome::Failed { failure, .. } => {
        Err(failure.into())
      }
    }
  }

  pub(crate) fn estimate(mut self) -> Result<TransactionEstimate, VirtualExecutionError> {
    match self.context.estimate(&self.transaction, false)? {
      EstimatedOutcome::Finished(result) => Ok(result.estimate),
      EstimatedOutcome::NotExecuted(failure) | EstimatedOutcome::Failed { failure, .. } => {
        Err(failure.into())
      }
    }
  }
}

impl SimulationContext<'_> {
  fn transact(
    &mut self,
    transaction: &SignedTransaction,
    collateral: ChargeCollateral,
    access_list: Option<AccessListObserver>,
  ) -> Result<ObservedOutcome, VirtualExecutionError> {
    // Saving also clears warmth from reads performed after the previous restore.
    let saved = self.state.save();
    let mut env = self.env.clone();
    env.transaction_hash = transaction.hash();
    let outcome = ExecutiveContext::new(&mut self.state, &env, self.machine, &self.spec).transact(
      transaction,
      TransactOptions {
        observer: ExecutionObserver::new(transaction.space(), access_list),
        settings: TransactSettings {
          charge_collateral: collateral,
          charge_gas: self.options.charge_gas(),
          check_base_price: self.options.has_price(),
          check_epoch_bound: false,
          forbid_eoa_with_code: false,
        },
      },
    )?;
    // Restore is only for further rounds. A database failure returns above;
    // the caller then drops the whole private State, including checkpoints.
    self.state.update_state_post_tx_execution(false);
    self.state.restore(saved);
    Ok(match outcome {
      ExecutionOutcome::NotExecutedDrop(error) => {
        ObservedOutcome::NotExecuted(SimulationFailure::Drop(error))
      }
      ExecutionOutcome::NotExecutedToReconsiderPacking(error) => {
        ObservedOutcome::NotExecuted(SimulationFailure::Repack(error))
      }
      ExecutionOutcome::ExecutionErrorBumpNonce(error, executed) => {
        ObservedOutcome::Failed(error, ObservedExecution::new(executed, &self.state)?)
      }
      ExecutionOutcome::Finished(executed) => {
        ObservedOutcome::Finished(ObservedExecution::new(executed, &self.state)?)
      }
    })
  }
}
