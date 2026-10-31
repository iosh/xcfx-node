use cfx_rpc_cfx_types::RpcAddress;
use cfx_types::{H128, Space};
use serde::Deserialize;
use serde_json::json;

use crate::{logs::PendingTransactionFilter, node::NativeNonceError};

use super::{
  connection::SubscriptionId,
  logs::{self, CoreLogFilter},
  operation::Operation,
  protocol::{self, RpcError},
  subscription::{self, CoreSubscription},
};

#[derive(Deserialize)]
pub(super) enum CoreRequest {
  #[serde(
    rename = "cfx_getLogs",
    deserialize_with = "protocol::deserialize_single"
  )]
  Logs(CoreLogFilter),
  #[serde(
    rename = "cfx_newFilter",
    deserialize_with = "protocol::deserialize_single"
  )]
  NewFilter(CoreLogFilter),
  #[serde(rename = "cfx_newBlockFilter")]
  NewBlockFilter,
  #[serde(rename = "cfx_newPendingTransactionFilter")]
  NewPendingTransactionFilter,
  #[serde(
    rename = "cfx_getFilterChanges",
    deserialize_with = "protocol::deserialize_single"
  )]
  FilterChanges(H128),
  #[serde(
    rename = "cfx_getFilterLogs",
    deserialize_with = "protocol::deserialize_single"
  )]
  FilterLogs(H128),
  #[serde(
    rename = "cfx_uninstallFilter",
    deserialize_with = "protocol::deserialize_single"
  )]
  UninstallFilter(H128),
  #[serde(rename = "cfx_subscribe")]
  Subscribe(CoreSubscription),
  #[serde(
    rename = "cfx_unsubscribe",
    deserialize_with = "protocol::deserialize_single"
  )]
  Unsubscribe(SubscriptionId),
  #[serde(
    rename = "txpool_nextNonce",
    deserialize_with = "protocol::deserialize_single"
  )]
  NextNonce(RpcAddress),
}

impl CoreRequest {
  pub(super) fn into_operation(self) -> Operation {
    match self {
      Self::Logs(filter) => logs::get_logs(Space::Native, filter.into_input()),
      Self::NewFilter(filter) => logs::new_filter(Space::Native, filter.into_input()),
      Self::NewBlockFilter => Operation::on_node(
        |node| node.new_block_filter(Space::Native),
        |id| Ok(json!(id)),
      ),
      Self::NewPendingTransactionFilter => Operation::on_node(
        |node| node.new_pending_transaction_filter(PendingTransactionFilter::Hashes(Space::Native)),
        |id| Ok(json!(id)),
      ),
      Self::FilterChanges(id) => logs::filter_changes(Space::Native, id),
      Self::FilterLogs(id) => logs::filter_logs(Space::Native, id),
      Self::UninstallFilter(id) => Operation::on_node(
        move |node| node.uninstall_filter(Space::Native, id),
        |removed| Ok(json!(removed)),
      ),
      Self::Subscribe(request) => request.into_operation(),
      Self::Unsubscribe(id) => subscription::unsubscribe(id),
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
