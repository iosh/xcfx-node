use serde::Serialize;
use serde_json::Value;

use crate::node::{NodeError, RequestError};

use super::request::Id;

/// Serialize directly to JSON bytes or text to preserve raw numeric IDs.
#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Response {
  Single(RpcResponse),
  Batch(Vec<RpcResponse>),
}

/// A response to one call. Notifications do not produce this value.
/// Serialize directly to JSON bytes or text to preserve raw numeric IDs.
#[derive(Serialize)]
pub(super) struct RpcResponse {
  jsonrpc: &'static str,
  id: Id,
  #[serde(flatten)]
  result: ResponseResult,
}

#[derive(Serialize)]
enum ResponseResult {
  #[serde(rename = "result")]
  Success(Value),
  #[serde(rename = "error")]
  Error(RpcError),
}

#[derive(Serialize)]
pub(super) struct RpcError {
  pub(super) code: i32,
  pub(super) message: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(super) data: Option<Value>,
}

impl RpcResponse {
  pub(super) fn new(id: Id, result: Result<Value, RpcError>) -> Self {
    Self {
      jsonrpc: "2.0",
      id,
      result: match result {
        Ok(value) => ResponseResult::Success(value),
        Err(error) => ResponseResult::Error(error),
      },
    }
  }
}

impl RpcError {
  pub(super) fn invalid_params() -> Self {
    Self {
      code: -32602,
      message: "Invalid params".into(),
      data: None,
    }
  }

  pub(super) fn method_not_found() -> Self {
    Self {
      code: -32601,
      message: "Method not found".into(),
      data: None,
    }
  }

  pub(super) fn internal() -> Self {
    Self {
      code: -32603,
      message: "Internal error".into(),
      data: None,
    }
  }

  pub(super) fn parse_error() -> Self {
    Self {
      code: -32700,
      message: "Parse error".into(),
      data: None,
    }
  }

  pub(super) fn invalid_request() -> Self {
    Self {
      code: -32600,
      message: "Invalid Request".into(),
      data: None,
    }
  }
}

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
