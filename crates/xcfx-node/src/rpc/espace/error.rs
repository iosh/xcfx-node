//! eSpace request, execution and query errors mapped to public RPC contracts.

use cfx_executor::executive::{ExecutionError, ToRepackError, TxDropError};
use cfx_rpc_eth_types::Bytes;
use cfx_vm_types::Error as VmError;
use primitives::transaction::TransactionError;
use serde_json::Value;

use crate::{
  fork::ForkReadError,
  rpc::protocol::RpcError,
  runtime::{EspaceStateQueryError, RuntimeSimulationError, RuntimeTransactionError},
  state::balance::BalanceChangeError,
  virtual_execution::{SimulationFailure, VirtualExecutionError},
};

pub(super) fn invalid_params(error: impl std::fmt::Display) -> RpcError {
  RpcError {
    code: -32602,
    message: error.to_string(),
    data: None,
  }
}

fn invalid_transaction(error: TransactionError) -> RpcError {
  let code = match error {
    TransactionError::ChainIdMismatch { .. }
    | TransactionError::NotEnoughBaseGas { .. }
    | TransactionError::GasLimitExceeded { .. }
    | TransactionError::InvalidGasLimit(_)
    | TransactionError::InsufficientGas { .. } => -32000,
    _ => -32003,
  };
  RpcError {
    code,
    message: error.to_string(),
    data: None,
  }
}

pub(super) fn transaction_error(error: RuntimeTransactionError) -> RpcError {
  match error {
    RuntimeTransactionError::Simulation(error) => virtual_execution_error(error),
    RuntimeTransactionError::Transaction(error) => invalid_transaction(error),
    RuntimeTransactionError::State(_) => processing_error("Transaction state access failed"),
    error @ (RuntimeTransactionError::SigningKeyNotFound { .. }
    | RuntimeTransactionError::NoSigningAccounts { .. }
    | RuntimeTransactionError::RawSpaceMismatch { .. }) => RpcError {
      code: -32602,
      message: error.to_string(),
      data: None,
    },
  }
}

pub(super) fn simulation_error(error: RuntimeSimulationError) -> RpcError {
  match error {
    RuntimeSimulationError::Selection(EspaceStateQueryError::Unavailable(error)) => RpcError {
      code: -32602,
      message: error.to_string(),
      data: None,
    },
    RuntimeSimulationError::Selection(error) => state_error(error),
    RuntimeSimulationError::Simulation(error) => virtual_execution_error(error),
  }
}

fn virtual_execution_error(error: VirtualExecutionError) -> RpcError {
  match error {
    error @ (VirtualExecutionError::InvalidNativeOverrideAddress(_)
    | VirtualExecutionError::InvalidFees { .. }) => invalid_params(error),
    VirtualExecutionError::Transaction(error) => invalid_transaction(error),
    VirtualExecutionError::State(_)
    | VirtualExecutionError::Balance(BalanceChangeError::State(_)) => {
      processing_error("Simulation state access failed")
    }
    VirtualExecutionError::Execution(error) => execution_error(error),
    error @ (VirtualExecutionError::Balance(_)
    | VirtualExecutionError::EpochHeightExhausted
    | VirtualExecutionError::StorageLimitOutOfRange { .. }) => RpcError {
      code: -32000,
      message: error.to_string(),
      data: None,
    },
  }
}

pub(super) fn execution_error(error: SimulationFailure) -> RpcError {
  match error {
    SimulationFailure::Execution {
      error: ExecutionError::VmError(VmError::Reverted),
      output,
      ..
    } => {
      let reason = solidity_abi::string_revert_reason_decode(&output);
      RpcError {
        code: 3,
        message: if reason.is_empty() {
          "execution reverted".into()
        } else {
          format!("execution reverted: {reason}")
        },
        data: Some(serde_json::json!(Bytes::new(output))),
      }
    }
    SimulationFailure::Execution {
      error: ExecutionError::VmError(error),
      ..
    } => RpcError {
      code: 3,
      message: format!("execution reverted: {error}"),
      data: Some(Value::String(String::new())),
    },
    SimulationFailure::Execution {
      error: ExecutionError::NonceOverflow(address),
      ..
    } => RpcError {
      code: 3,
      message: format!("address nonce overflow: {address:?}"),
      data: Some(Value::String(String::new())),
    },
    SimulationFailure::Execution {
      error: ExecutionError::NotEnoughCash { .. },
      ..
    }
    | SimulationFailure::Repack(ToRepackError::SenderDoesNotExist) => RpcError {
      code: -32003,
      message: "insufficient funds for gas * price + value".into(),
      data: None,
    },
    SimulationFailure::Drop(TxDropError::NotEnoughGasLimit { expected, got }) => RpcError {
      code: -32000,
      message: format!("not enough gas: required {expected}, got {got}"),
      data: None,
    },
    error => RpcError {
      code: -32000,
      message: error.to_string(),
      data: None,
    },
  }
}

pub(super) fn state_error(error: EspaceStateQueryError) -> RpcError {
  match error {
    EspaceStateQueryError::Unavailable(_) | EspaceStateQueryError::UnknownBlock(_) => {
      RpcError::invalid_params()
    }
    EspaceStateQueryError::Fork(error) => fork_error(error),
    EspaceStateQueryError::State(_) => processing_error("State query failed"),
  }
}

pub(super) fn fork_error(error: ForkReadError) -> RpcError {
  match error {
    ForkReadError::Closed => RpcError {
      code: -32000,
      message: "Node is closed".into(),
      data: None,
    },
    ForkReadError::ServiceStopped => RpcError::internal(),
    _ => processing_error("Remote query failed"),
  }
}

fn processing_error(message: &str) -> RpcError {
  // Conflux maps state/storage processing failures to EXCEPTION_ERROR.
  RpcError {
    code: -32016,
    message: message.into(),
    data: None,
  }
}
