//! JSON-RPC response shapes and standard protocol errors.

use serde::Serialize;
use serde_json::Value;

use super::request::Id;

/// Serialize directly to JSON bytes or text to preserve raw numeric IDs.
#[derive(Serialize)]
#[serde(untagged)]
pub(in crate::rpc) enum Response {
  Single(RpcResponse),
  Batch(Vec<RpcResponse>),
}

/// A response to one call. Notifications do not produce this value.
/// Serialize directly to JSON bytes or text to preserve raw numeric IDs.
#[derive(Serialize)]
pub(in crate::rpc) struct RpcResponse {
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
pub(in crate::rpc) struct RpcError {
  pub(in crate::rpc) code: i32,
  pub(in crate::rpc) message: String,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub(in crate::rpc) data: Option<Value>,
}

impl RpcResponse {
  pub(in crate::rpc) fn new(id: Id, result: Result<Value, RpcError>) -> Self {
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
  pub(in crate::rpc) fn invalid_params() -> Self {
    Self {
      code: -32602,
      message: "Invalid params".into(),
      data: None,
    }
  }

  pub(in crate::rpc) fn method_not_found() -> Self {
    Self {
      code: -32601,
      message: "Method not found".into(),
      data: None,
    }
  }

  pub(in crate::rpc) fn internal() -> Self {
    Self {
      code: -32603,
      message: "Internal error".into(),
      data: None,
    }
  }

  pub(in crate::rpc) fn parse_error() -> Self {
    Self {
      code: -32700,
      message: "Parse error".into(),
      data: None,
    }
  }

  pub(in crate::rpc) fn invalid_request() -> Self {
    Self {
      code: -32600,
      message: "Invalid Request".into(),
      data: None,
    }
  }
}
