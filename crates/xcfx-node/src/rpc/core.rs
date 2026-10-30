use cfx_rpc_cfx_types::RpcAddress;
use serde::Deserialize;
use serde_json::json;

use crate::node::NativeNonceError;

use super::{
  operation::Operation,
  protocol::{self, RpcError},
};

#[derive(Deserialize)]
pub(super) enum CoreRequest {
  #[serde(
    rename = "txpool_nextNonce",
    deserialize_with = "protocol::deserialize_single"
  )]
  NextNonce(RpcAddress),
}

impl CoreRequest {
  pub(super) fn into_operation(self) -> Operation {
    match self {
      Self::NextNonce(address) => Operation::on_node(
        move |node| node.native_pending_nonce(address),
        |result| {
          result
            .map(|nonce| json!(nonce))
            .map_err(|error| match error {
              NativeNonceError::Network(error) => RpcError {
                code: -32602,
                message: error.to_string(),
                data: None,
              },
              NativeNonceError::State(_) => RpcError {
                code: -32016,
                message: "State query failed".into(),
                data: None,
              },
            })
        },
      ),
    }
  }
}
