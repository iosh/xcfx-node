//! Wire filters and log results; history selection remains in the Runtime.

use cfx_addr::Network;
use cfx_rpc_cfx_types::{CfxRpcLogFilter, Log as CoreLog};
use cfx_rpc_eth_types::{BlockId, EthRpcLogFilter, Log as EspaceLog, LogData};
use cfx_types::{H128, Space};
use primitives::{
  EpochNumber,
  filter::{FilterError, LogFilter, LogFilterParams},
  log_entry::LocalizedLogEntry,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
  fork::ForkReadError,
  logs::{FilterChanges, LogFilterInput, LogQueryError},
};

use super::{
  espace::convert,
  operation::{Operation, RpcResult},
  protocol::RpcError,
};

#[derive(Deserialize)]
#[serde(try_from = "EthRpcLogFilter")]
pub(super) struct EspaceLogFilter {
  input: LogFilterInput,
  to_pending: bool,
}

impl EspaceLogFilter {
  pub(super) fn into_input(self) -> LogFilterInput {
    self.input
  }

  pub(super) fn into_new_filter(self) -> Operation {
    if self.to_pending {
      return Operation::Immediate(Err(RpcError {
        code: -32600,
        message: "Filter logs from pending blocks is not supported".into(),
        data: None,
      }));
    }
    new_filter(Space::Ethereum, self.input)
  }
}

impl TryFrom<EthRpcLogFilter> for EspaceLogFilter {
  type Error = String;

  fn try_from(filter: EthRpcLogFilter) -> Result<Self, Self::Error> {
    let to_pending = filter.to_block == Some(BlockId::Pending);
    if filter
      .topics
      .as_ref()
      .is_some_and(|topics| topics.len() > 4)
    {
      return Err("expected at most four topic filters".into());
    }
    let params = LogFilterParams {
      address: filter.address.map(|address| address.to_vec()),
      topics: filter
        .topics
        .unwrap_or_default()
        .into_iter()
        .map(|topic| topic.to_opt())
        .collect(),
      trusted: false,
      space: Space::Ethereum,
    };
    let filter = match (filter.from_block, filter.to_block, filter.block_hash) {
      (None, None, Some(hash)) => LogFilter::BlockHashLogFilter {
        block_hashes: vec![hash],
        params,
      },
      (from, to, None) => LogFilter::EpochLogFilter {
        from_epoch: from
          .map(EpochNumber::try_from)
          .transpose()
          .map_err(|error| error.to_string())?
          .unwrap_or(EpochNumber::LatestState),
        to_epoch: to
          .map(EpochNumber::try_from)
          .transpose()
          .map_err(|error| error.to_string())?
          .unwrap_or(EpochNumber::LatestState),
        params,
      },
      _ => return Err("blockHash cannot be combined with fromBlock or toBlock".into()),
    };
    Ok(Self {
      input: LogFilterInput::new(filter, None),
      to_pending,
    })
  }
}

#[derive(Deserialize)]
#[serde(try_from = "CfxRpcLogFilter")]
pub(super) struct CoreLogFilter(LogFilterInput);

impl CoreLogFilter {
  pub(super) fn into_input(self) -> LogFilterInput {
    self.0
  }
}

impl TryFrom<CfxRpcLogFilter> for CoreLogFilter {
  type Error = String;

  fn try_from(filter: CfxRpcLogFilter) -> Result<Self, Self::Error> {
    let mut network = None;
    if let Some(addresses) = &filter.address {
      for address in addresses.iter() {
        if network
          .replace(address.network)
          .is_some_and(|previous| previous != address.network)
        {
          return Err("filter addresses must use the same network".into());
        }
      }
    }
    let filter = filter
      .into_primitive()
      .map_err(|error| error.message().to_owned())?;
    Ok(Self(LogFilterInput::new(filter, network)))
  }
}

pub(super) fn default_filter(space: Space) -> LogFilterInput {
  let mut filter = LogFilter::default();
  filter.space = space;
  LogFilterInput::new(filter, None)
}

pub(super) fn get_logs(space: Space, input: LogFilterInput) -> Operation {
  Operation::on_node(
    move |node| node.logs(input),
    move |result| log_result(space, result),
  )
}

pub(super) fn new_filter(space: Space, input: LogFilterInput) -> Operation {
  Operation::on_node(
    move |node| node.new_log_filter(input),
    move |result| {
      result
        .map(|id| json!(id))
        .map_err(|error| query_error(space, error))
    },
  )
}

pub(super) fn filter_logs(space: Space, id: H128) -> Operation {
  Operation::on_node(
    move |node| node.filter_logs(space, id),
    move |result| log_result(space, result),
  )
}

pub(super) fn filter_changes(space: Space, id: H128) -> Operation {
  Operation::on_node(
    move |node| node.filter_changes(space, id),
    move |result| {
      let (network, changes) = result.map_err(|error| query_error(space, error))?;
      match changes {
        FilterChanges::Hashes(hashes) => Ok(json!(hashes)),
        FilterChanges::EspaceTransactions(transactions) => Ok(json!(
          transactions
            .iter()
            .map(convert::pending_transaction)
            .collect::<Vec<_>>()
        )),
        FilterChanges::Logs(changes) => {
          let mut logs = Vec::new();
          match space {
            Space::Native => {
              // Core polling uses the first replaced epoch; Core subscriptions
              // use the preceding epoch. Keep these wire contracts separate.
              if let Some(epoch) = changes.reverted_from {
                logs.push(json!({ "revertTo": cfx_types::U256::from(epoch) }));
              }
            }
            Space::Ethereum => {
              logs.extend(
                changes
                  .removed
                  .into_iter()
                  .map(|log| log_value(space, network, log, true)),
              );
            }
          }
          logs.extend(
            changes
              .added
              .into_iter()
              .map(|log| log_value(space, network, log, false)),
          );
          Ok(Value::Array(logs))
        }
      }
    },
  )
}

fn log_result(
  space: Space,
  result: Result<(Network, Vec<LocalizedLogEntry>), LogQueryError>,
) -> RpcResult {
  let (network, logs) = result.map_err(|error| query_error(space, error))?;
  Ok(Value::Array(
    logs
      .into_iter()
      .map(|log| log_value(space, network, log, false))
      .collect(),
  ))
}

pub(super) fn log_value(
  space: Space,
  network: Network,
  log: LocalizedLogEntry,
  removed: bool,
) -> Value {
  assert_eq!(
    space, log.entry.space,
    "a log projection retains its selected Space"
  );
  match space {
    Space::Native => json!(
      CoreLog::try_from_localized(log, network)
        .expect("local and validated remote Core logs have encodable addresses")
    ),
    Space::Ethereum => json!(EspaceLog {
      inner: LogData {
        address: log.entry.address,
        topics: log.entry.topics,
        data: log.entry.data.into()
      },
      block_hash: log.block_hash,
      block_number: log.epoch_number.into(),
      block_timestamp: log.block_timestamp.map(Into::into),
      transaction_hash: log.transaction_hash,
      transaction_index: log.transaction_index.into(),
      log_index: Some(log.log_index.into()),
      transaction_log_index: Some(log.transaction_log_index.into()),
      removed,
    }),
  }
}

pub(super) fn query_error(space: Space, error: LogQueryError) -> RpcError {
  let code = match &error {
    LogQueryError::FilterNotFound => -32600,
    LogQueryError::Network { .. } => -32602,
    LogQueryError::Filter(FilterError::UnknownBlock { .. }) if space == Space::Ethereum => -32602,
    LogQueryError::Fork(ForkReadError::Closed) => {
      return RpcError {
        code: -32000,
        message: "Node is closed".into(),
        data: None,
      };
    }
    LogQueryError::Fork(ForkReadError::ServiceStopped) => return RpcError::internal(),
    LogQueryError::Fork(_) => {
      return RpcError {
        code: -32016,
        message: "Remote query failed".into(),
        data: None,
      };
    }
    _ => -32016,
  };
  RpcError {
    code,
    message: error.to_string(),
    data: None,
  }
}
