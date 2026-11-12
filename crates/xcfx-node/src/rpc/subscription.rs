//! Connection-owned projections of ordered node facts.

use std::{ops::ControlFlow, sync::Arc};

use cfx_addr::Network;
use cfx_rpc_cfx_types::Header as CoreHeader;
use cfx_types::{Space, U256};
use primitives::filter::LogFilterParams;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
  chain::EpochView,
  logs::{EpochCursor, LogFilterInput, LogIndexScope, PendingTransactionFilter, visit_local_logs},
  node::{Observation, ObservationRequest},
  runtime::ChainChange,
  runtime_transaction::RuntimeTransaction,
};

use super::{
  connection::{ConnectionCall, SubscriptionId},
  espace::convert,
  logs::{CoreLogFilter, EspaceLogFilter, default_filter, log_value},
  operation::{Operation, RpcResult},
  protocol::RpcError,
};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EpochSubscription {
  LatestMined,
  LatestState,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum EspaceKind {
  NewHeads,
  Logs,
  NewPendingTransactions,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum CoreKind {
  NewHeads,
  Logs,
  NewPendingTransactions,
  Epochs,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EspaceParams {
  Logs(EspaceLogFilter),
  FullTransactions(bool),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CoreParams {
  Logs(CoreLogFilter),
  Epochs(EpochSubscription),
}

#[derive(Deserialize)]
struct EspaceArguments(EspaceKind, #[serde(default)] Option<EspaceParams>);

#[derive(Deserialize)]
struct CoreArguments(CoreKind, #[serde(default)] Option<CoreParams>);

// These newtypes decode the complete positional argument array, including its
// optional second argument. Invalid kind/parameter combinations never run.
#[derive(Deserialize)]
#[serde(try_from = "EspaceArguments")]
pub(super) struct EspaceSubscription(SubscriptionRequest);

#[derive(Deserialize)]
#[serde(try_from = "CoreArguments")]
pub(super) struct CoreSubscription(SubscriptionRequest);

enum SubscriptionRequest {
  Heads(Space),
  Epochs(EpochSubscription),
  Logs(LogFilterInput),
  Pending(PendingTransactionFilter),
}

impl TryFrom<EspaceArguments> for EspaceSubscription {
  type Error = &'static str;
  fn try_from(EspaceArguments(kind, params): EspaceArguments) -> Result<Self, Self::Error> {
    let request = match (kind, params) {
      (EspaceKind::NewHeads, None) => SubscriptionRequest::Heads(Space::Ethereum),
      (EspaceKind::Logs, None) => SubscriptionRequest::Logs(default_filter(Space::Ethereum)),
      (EspaceKind::Logs, Some(EspaceParams::Logs(filter))) => {
        SubscriptionRequest::Logs(filter.into_input())
      }
      (EspaceKind::NewPendingTransactions, None | Some(EspaceParams::FullTransactions(false))) => {
        SubscriptionRequest::Pending(PendingTransactionFilter::Hashes(Space::Ethereum))
      }
      (EspaceKind::NewPendingTransactions, Some(EspaceParams::FullTransactions(true))) => {
        SubscriptionRequest::Pending(PendingTransactionFilter::FullEspace)
      }
      _ => return Err("invalid subscription parameters"),
    };
    Ok(Self(request))
  }
}

impl TryFrom<CoreArguments> for CoreSubscription {
  type Error = &'static str;
  fn try_from(CoreArguments(kind, params): CoreArguments) -> Result<Self, Self::Error> {
    let request = match (kind, params) {
      (CoreKind::NewHeads, None) => SubscriptionRequest::Heads(Space::Native),
      (CoreKind::Logs, None) => SubscriptionRequest::Logs(default_filter(Space::Native)),
      (CoreKind::Logs, Some(CoreParams::Logs(filter))) => {
        SubscriptionRequest::Logs(filter.into_input())
      }
      (CoreKind::Epochs, None) => SubscriptionRequest::Epochs(EpochSubscription::LatestMined),
      (CoreKind::Epochs, Some(CoreParams::Epochs(epoch))) => SubscriptionRequest::Epochs(epoch),
      (CoreKind::NewPendingTransactions, None) => {
        SubscriptionRequest::Pending(PendingTransactionFilter::Hashes(Space::Native))
      }
      _ => return Err("invalid subscription parameters"),
    };
    Ok(Self(request))
  }
}

impl EspaceSubscription {
  pub(super) fn into_operation(self) -> Operation {
    self.0.into_operation()
  }
}

impl CoreSubscription {
  pub(super) fn into_operation(self) -> Operation {
    self.0.into_operation()
  }
}

impl SubscriptionRequest {
  fn into_operation(self) -> Operation {
    match self {
      Self::Logs(input) => Operation::on_connection(
        ObservationRequest::Logs(input),
        |connection, observation| {
          let Observation::Logs {
            network,
            filter,
            visible_epoch,
          } = observation
          else {
            unreachable!("a log observation returns its validated filter and installation position")
          };
          install(
            connection,
            SubscriptionEvents::Logs {
              network,
              filter,
              cursor: EpochCursor::new(visible_epoch),
            },
          )
        },
      ),
      request => {
        Operation::on_connection(ObservationRequest::Chain, move |connection, observation| {
          let Observation::Chain {
            network,
            mined_epoch,
            visible_epoch,
          } = observation
          else {
            unreachable!("a chain observation returns the installation positions")
          };
          let events = match request {
            Self::Heads(Space::Native) => SubscriptionEvents::CoreHeads { network },
            Self::Heads(Space::Ethereum) => SubscriptionEvents::EspaceHeads {
              cursor: EpochCursor::new(visible_epoch),
            },
            Self::Epochs(source) => SubscriptionEvents::Epochs {
              source,
              cursor: EpochCursor::new(match source {
                EpochSubscription::LatestMined => mined_epoch,
                EpochSubscription::LatestState => visible_epoch,
              }),
            },
            Self::Pending(kind) => SubscriptionEvents::Pending(kind),
            Self::Logs(_) => unreachable!("log subscriptions use a log observation"),
          };
          install(connection, events)
        })
      }
    }
  }
}

fn install(connection: ConnectionCall<'_>, events: SubscriptionEvents) -> RpcResult {
  connection
    .subscribe(events)
    .map(|id| json!(id))
    .ok_or_else(|| RpcError {
      code: -32601,
      message: "Subscriptions are not available on this connection".into(),
      data: None,
    })
}

pub(super) fn unsubscribe(id: SubscriptionId) -> Operation {
  Operation::on_connection(ObservationRequest::Chain, move |connection, _| {
    Ok(json!(connection.unsubscribe(id)))
  })
}

pub(super) enum SubscriptionEvents {
  CoreHeads {
    network: Network,
  },
  EspaceHeads {
    cursor: EpochCursor,
  },
  Epochs {
    source: EpochSubscription,
    cursor: EpochCursor,
  },
  Logs {
    network: Network,
    filter: LogFilterParams,
    cursor: EpochCursor,
  },
  Pending(PendingTransactionFilter),
}

impl SubscriptionEvents {
  /// Returning false terminates only this subscription and releases its queue.
  pub(super) fn chain_changed(
    &mut self,
    change: &ChainChange,
    emit: &mut dyn FnMut(Value) -> bool,
  ) -> bool {
    match self {
      Self::CoreHeads { .. } | Self::Pending(_) => {}
      Self::EspaceHeads { cursor } => {
        cursor.retract(change);
        for view in change.visible_history().added() {
          if cursor.report(view.epoch_height())
            && !emit(json!(convert::local_header(
              &view
                .espace_block()
                .expect("a newly visible local epoch has an eSpace projection")
            )))
          {
            return false;
          }
        }
      }
      Self::Epochs { source, cursor } => {
        cursor.retract(change);
        let history = match source {
          EpochSubscription::LatestMined => change.history(),
          EpochSubscription::LatestState => change.visible_history(),
        };
        for view in history.added() {
          if cursor.report(view.epoch_height()) {
            let hashes: Vec<_> = view
              .artifacts()
              .expect("new epochs have local artifacts")
              .all_blocks()
              .map(|block| block.hash())
              .collect();
            if !emit(
              json!({ "epochNumber": U256::from(view.epoch_height()), "epochHashesOrdered": hashes }),
            ) {
              return false;
            }
          }
        }
      }
      Self::Logs {
        network,
        filter,
        cursor,
      } => {
        let removed = cursor.retract(change);
        match filter.space {
          Space::Native => {
            if !removed.is_empty() {
              let revert_to = change.history().common_ancestor().epoch_height();
              if !emit(json!({ "revertTo": U256::from(revert_to) })) {
                return false;
              }
            }
          }
          Space::Ethereum => {
            for view in &removed {
              if !emit_epoch_logs(view, filter, *network, true, emit) {
                return false;
              }
            }
          }
        }
        for view in change.visible_history().added() {
          if cursor.report(view.epoch_height())
            && !emit_epoch_logs(view, filter, *network, false, emit)
          {
            return false;
          }
        }
      }
    }
    true
  }

  pub(super) fn block_mined(
    &mut self,
    view: &EpochView,
    emit: &mut dyn FnMut(Value) -> bool,
  ) -> bool {
    match self {
      Self::CoreHeads { network } => emit(json!(core_header(view, *network))),
      _ => true,
    }
  }

  pub(super) fn transaction_inserted(
    &mut self,
    transaction: &RuntimeTransaction,
    emit: &mut dyn FnMut(Value) -> bool,
  ) -> bool {
    match self {
      Self::Pending(PendingTransactionFilter::Hashes(space)) if *space == transaction.space() => {
        emit(json!(transaction.hash()))
      }
      Self::Pending(PendingTransactionFilter::FullEspace)
        if transaction.space() == Space::Ethereum =>
      {
        emit(json!(convert::pending_transaction(transaction)))
      }
      _ => true,
    }
  }
}

fn emit_epoch_logs(
  view: &Arc<EpochView>,
  filter: &LogFilterParams,
  network: Network,
  removed: bool,
  emit: &mut dyn FnMut(Value) -> bool,
) -> bool {
  visit_local_logs(view, filter, LogIndexScope::Epoch, |log| {
    if emit(log_value(filter.space, network, log, removed)) {
      ControlFlow::Continue(())
    } else {
      ControlFlow::Break(())
    }
  })
  .is_continue()
}

fn core_header(view: &EpochView, network: Network) -> CoreHeader {
  let block = view
    .artifacts()
    .expect("a mined block has local artifacts")
    .pivot_runtime_block();
  super::core::convert::header(
    block,
    Some(view.epoch_height()),
    Some(view.pivot_block_number()),
    network,
  )
}
