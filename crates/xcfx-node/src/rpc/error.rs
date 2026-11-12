//! Shared lifecycle, cancellation, and state-query error mappings.

use crate::{
  fork::ForkReadError,
  node::{NodeError, RequestError},
  runtime::{BlockPreparationError, FeeQueryError, StateQueryError, StateSelectionError},
};

use super::protocol::RpcError;

impl From<&RequestError> for RpcError {
  fn from(error: &RequestError) -> Self {
    match error {
      RequestError::Cancelled => Self {
        code: -32000,
        message: "Request cancelled; completed changes are not rolled back".into(),
        data: None,
      },
      RequestError::Node(NodeError::Closed) => Self {
        code: -32000,
        message: "Node is closed".into(),
        data: None,
      },
      RequestError::Node(_) => Self::internal(),
    }
  }
}

pub(super) fn state_error(error: StateQueryError) -> RpcError {
  match error {
    StateQueryError::Selection(error) => state_selection_error(error),
    StateQueryError::Fork(error) => fork_error(error),
    StateQueryError::State(_) => processing_error("State query failed"),
  }
}

pub(super) fn state_selection_error(error: StateSelectionError) -> RpcError {
  match error {
    StateSelectionError::Unavailable(_) | StateSelectionError::UnknownBlock(_) => {
      RpcError::invalid_params()
    }
    StateSelectionError::Fork(error) => fork_error(error),
  }
}

pub(super) fn fee_query_error(error: FeeQueryError) -> RpcError {
  match error {
    FeeQueryError::UnknownBlock => RpcError::invalid_params(),
    FeeQueryError::Fork(error) => fork_error(error),
    FeeQueryError::Preparation(BlockPreparationError::State(error)) => state_error(error.into()),
    error @ (FeeQueryError::Preparation(BlockPreparationError::Time(_))
    | FeeQueryError::Preparation(BlockPreparationError::PositionExhausted(_))
    | FeeQueryError::SuggestionOverflow) => RpcError {
      code: -32000,
      message: error.to_string(),
      data: None,
    },
  }
}

pub(super) fn fork_error(error: ForkReadError) -> RpcError {
  match error {
    ForkReadError::PivotAssumption => RpcError {
      code: -32602,
      message: "pivot chain assumption failed".into(),
      data: None,
    },
    ForkReadError::Closed => RpcError {
      code: -32000,
      message: "Node is closed".into(),
      data: None,
    },
    ForkReadError::ServiceStopped => RpcError::internal(),
    _ => processing_error("Remote query failed"),
  }
}

pub(super) fn processing_error(message: &str) -> RpcError {
  // Conflux maps state/storage processing failures to EXCEPTION_ERROR.
  RpcError {
    code: -32016,
    message: message.into(),
    data: None,
  }
}
