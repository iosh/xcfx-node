//! Core RPC error mappings for queries, transactions, and simulation.

use cfx_addr::Network;
use cfx_executor::executive::ExecutionError;
use cfx_rpc_cfx_types::Bytes;
use cfx_vm_types::Error as VmError;
use serde_json::json;

use crate::{
  node::{
    CoreBlockQueryError, CoreEpochReceiptsError, CoreOperationError, CoreStateQueryError,
    NativeNonceError,
  },
  rpc::{
    error::{fork_error, processing_error, state_error, state_selection_error},
    protocol::RpcError,
  },
  runtime::{
    CoreBlockSelectionError, CoreEpochQueryError, CoreStorageRootError, RuntimeSimulationError,
    RuntimeTransactionError, StateQueryError,
  },
  signing::MessageSigningError,
  state::balance::BalanceChangeError,
  virtual_execution::{SimulationFailure, VirtualExecutionError},
};

pub(super) fn invalid(error: impl std::fmt::Display) -> RpcError {
  RpcError {
    code: -32602,
    message: error.to_string(),
    data: None,
  }
}

fn operation<E>(
  error: CoreOperationError<E>,
  convert: impl FnOnce(E, Network) -> RpcError,
) -> RpcError {
  match error {
    CoreOperationError::Network(error) => invalid(error),
    CoreOperationError::Operation { error, network } => convert(error, network),
  }
}

pub(super) fn state<E: Into<StateQueryError>>(error: CoreStateQueryError<E>) -> RpcError {
  match error {
    CoreStateQueryError::Network(error) => invalid(error),
    CoreStateQueryError::Query(error) => state_error(error.into()),
  }
}

pub(super) fn storage_root(error: CoreStateQueryError<CoreStorageRootError>) -> RpcError {
  match error {
    CoreStateQueryError::Network(error) => invalid(error),
    CoreStateQueryError::Query(CoreStorageRootError::Unavailable(_)) => RpcError::invalid_params(),
    CoreStateQueryError::Query(error @ CoreStorageRootError::Unsupported) => RpcError {
      code: -32000,
      message: error.to_string(),
      data: None,
    },
  }
}

pub(super) fn epoch(error: CoreEpochQueryError) -> RpcError {
  match error {
    CoreEpochQueryError::Epoch(_) => RpcError::invalid_params(),
    CoreEpochQueryError::Fork(error) => fork_error(error),
  }
}

pub(super) fn block(error: CoreBlockQueryError) -> RpcError {
  match error {
    CoreBlockQueryError::Selection(CoreBlockSelectionError::Epoch(_)) => RpcError::invalid_params(),
    CoreBlockQueryError::Selection(error @ CoreBlockSelectionError::PivotAssumption) => {
      invalid(error)
    }
    CoreBlockQueryError::Fork(error) => fork_error(error),
  }
}

pub(super) fn epoch_receipts(error: CoreEpochReceiptsError) -> RpcError {
  match error {
    CoreEpochReceiptsError::Selection(error) => state_selection_error(error),
    CoreEpochReceiptsError::Fork(error) => fork_error(error),
  }
}

pub(super) fn pending_nonce(error: NativeNonceError) -> RpcError {
  match error {
    NativeNonceError::Network(error) => invalid(error),
    NativeNonceError::State(error) => state_error(error.into()),
  }
}

pub(super) fn transaction(error: CoreOperationError<RuntimeTransactionError>) -> RpcError {
  operation(error, transaction_error)
}

pub(super) fn raw_transaction(error: RuntimeTransactionError) -> RpcError {
  match error {
    RuntimeTransactionError::Fork(error) => crate::rpc::error::fork_error(error),
    RuntimeTransactionError::State(_) => processing_error("Transaction state access failed"),
    error => invalid(error),
  }
}

fn transaction_error(error: RuntimeTransactionError, network: Network) -> RpcError {
  match error {
    RuntimeTransactionError::Fork(error) => crate::rpc::error::fork_error(error),
    RuntimeTransactionError::Simulation(error) => virtual_error(error, network, true),
    RuntimeTransactionError::State(_) => processing_error("Transaction state access failed"),
    error => invalid(error),
  }
}

pub(super) fn signing(error: CoreOperationError<MessageSigningError>) -> RpcError {
  operation(error, |_, _| RpcError::internal())
}

pub(super) fn call(error: CoreOperationError<RuntimeSimulationError>) -> RpcError {
  operation(error, |error, network| simulation(error, network, false))
}

pub(super) fn estimate(error: CoreOperationError<RuntimeSimulationError>) -> RpcError {
  operation(error, |error, network| simulation(error, network, true))
}

fn simulation(error: RuntimeSimulationError, network: Network, estimating: bool) -> RpcError {
  match error {
    RuntimeSimulationError::Selection(error) => state_selection_error(error),
    RuntimeSimulationError::Simulation(error) => virtual_error(error, network, estimating),
  }
}

fn execution_error(message: String, data: String) -> RpcError {
  RpcError {
    code: -32015,
    message,
    data: Some(json!(data)),
  }
}

fn virtual_error(error: VirtualExecutionError, network: Network, estimating: bool) -> RpcError {
  match error {
    VirtualExecutionError::State(_)
    | VirtualExecutionError::Balance(BalanceChangeError::State(_)) => {
      processing_error("Simulation state access failed")
    }
    VirtualExecutionError::Execution(failure) => match failure {
      SimulationFailure::Execution {
        error: ExecutionError::VmError(VmError::Reverted),
        output,
        errors,
      } => {
        if !estimating {
          return RpcError {
            code: -32015,
            message: "Transaction reverted".into(),
            data: Some(json!(Bytes::new(output))),
          };
        }
        let reason = solidity_abi::string_revert_reason_decode(&output);
        let suffix = if reason.is_empty() {
          String::new()
        } else {
          format!(": {reason}")
        };
        let errors = errors
          .expect("Core estimation failures retain their Native error observations")
          .into_iter()
          .map(|(address, error)| {
            format!(
              "{}: {error}",
              super::convert::address(address, network).base32_address
            )
          })
          .collect::<Vec<_>>();
        let innermost = errors.first().map_or_else(String::new, |error| {
          format!(" Innermost error is at {error}.")
        });
        execution_error(
          format!("Estimation isn't accurate: transaction is reverted{suffix}{innermost}"),
          errors.join("\n"),
        )
      }
      SimulationFailure::Execution { error, .. } => execution_error(
        if estimating {
          format!(
            "Can not estimate: transaction execution failed, all gas will be charged (execution error: {error:?})"
          )
        } else {
          "Transaction execution failed".into()
        },
        format!("{error:?}"),
      ),
      error => execution_error(
        if estimating {
          "Can not estimate: transaction can not be executed".into()
        } else {
          "Transaction can not be executed".into()
        },
        error.to_string(),
      ),
    },
    VirtualExecutionError::Transaction(error) => RpcError {
      code: -32602,
      message: "Invalid parameters: tx".into(),
      data: Some(json!(error.to_string())),
    },
    error @ (VirtualExecutionError::InvalidNativeOverrideAddress(_)
    | VirtualExecutionError::InvalidFees { .. }) => invalid(error),
    error => processing_error(&error.to_string()),
  }
}
