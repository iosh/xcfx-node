//! Maps node lifecycle and cancellation failures to protocol errors.

use crate::node::{NodeError, RequestError};

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
