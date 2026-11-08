mod query;

use cfx_rpc_cfx_types::{BlockHashOrEpochNumber, EpochNumber, RpcAddress};
use cfx_types::{BigEndianHash, H128, H256, Space, U256};
use serde::Deserialize;
use serde_json::json;

use crate::logs::PendingTransactionFilter;

use super::{
  connection::SubscriptionId,
  logs::{self, CoreLogFilter},
  operation::Operation,
  protocol,
  subscription::{self, CoreSubscription},
};

#[derive(Deserialize)]
pub(super) enum CoreRequest {
  #[serde(
    rename = "cfx_epochNumber",
    deserialize_with = "protocol::deserialize_optional"
  )]
  EpochNumber(Option<EpochNumber>),
  #[serde(rename = "cfx_getBalance")]
  Balance(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getNextNonce")]
  Nonce(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getCode")]
  Code(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getStorageAt")]
  Storage(
    RpcAddress,
    U256,
    #[serde(default)] Option<BlockHashOrEpochNumber>,
  ),
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
  PendingNonce(RpcAddress),
}

impl CoreRequest {
  pub(super) fn into_operation(self) -> Operation {
    match self {
      Self::EpochNumber(epoch) => query::epoch_number(epoch),
      Self::Balance(address, selector) => {
        let selector = query::state_selector(selector);
        Operation::on_node(
          move |node| node.core_balance_at(address, selector),
          query::state,
        )
      }
      Self::Nonce(address, selector) => {
        let selector = query::state_selector(selector);
        Operation::on_node(
          move |node| node.core_nonce_at(address, selector),
          query::state,
        )
      }
      Self::Code(address, selector) => {
        let selector = query::state_selector(selector);
        Operation::on_node(
          move |node| node.core_code_at(address, selector),
          query::code,
        )
      }
      Self::Storage(address, position, selector) => {
        let selector = query::state_selector(selector);
        let position = H256::from_uint(&position);
        Operation::on_node(
          move |node| node.core_storage_at(address, position, selector),
          query::state,
        )
      }
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
      Self::PendingNonce(address) => Operation::on_node(
        move |node| node.native_pending_nonce(address),
        query::pending_nonce,
      ),
    }
  }
}
