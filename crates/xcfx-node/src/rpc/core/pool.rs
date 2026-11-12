//! Core debug uses the Runtime's complete ready prefixes, with deferred including ready.

use std::collections::BTreeMap;

use cfx_addr::Network;
use cfx_rpc_cfx_types::{
  AccountPendingInfo, AccountPendingTransactions, PendingReason, TransactionStatus,
  TxPoolPendingNonceRange, TxPoolStatus, TxWithPoolInfo, address::UnexpectedRpcAddressNetwork,
};
use cfx_statedb::Result as StateResult;
use cfx_types::{H256, U64, U256};
use primitives::Action;
use serde_json::json;

use crate::{
  node::CoreStateQueryError,
  rpc::{error::state_error, operation::RpcResult},
  runtime::{
    AccountPoolTransactions, CorePoolStatus, PoolAccountInfo, PoolContent, PoolTransactionInfo,
    PoolTransactionStatus, QueuedReason,
  },
  runtime_transaction::RuntimeTransaction,
};

use super::{convert::pending_transaction, error};

pub(super) fn status(result: StateResult<CorePoolStatus>) -> RpcResult {
  let (ready, retained) = result
    .map_err(|error| state_error(error.into()))?
    .into_parts();

  Ok(json!(TxPoolStatus {
    ready: ready.into(),
    deferred: retained.into(),
    // The upstream counter has no increment path. Locally executed entries leave the pool.
    received: U64::zero(),
    unexecuted: retained.into(),
  }))
}

pub(super) fn nonce_range(
  result: Result<Option<(U256, U256)>, UnexpectedRpcAddressNetwork>,
) -> RpcResult {
  let (min_nonce, max_nonce) = result
    .map_err(error::invalid)?
    .unwrap_or((U256::MAX, U256::zero()));

  Ok(json!(TxPoolPendingNonceRange {
    min_nonce,
    max_nonce,
  }))
}

pub(super) fn account_info(
  result: Result<PoolAccountInfo, CoreStateQueryError<cfx_statedb::Error>>,
) -> RpcResult {
  let (local_nonce, pending_count, first) = result.map_err(error::state)?.into_parts();
  let (pending_nonce, next_pending_tx) = first.unwrap_or((U256::zero(), H256::zero()));

  Ok(json!(AccountPendingInfo {
    local_nonce,
    pending_count: pending_count.into(),
    pending_nonce,
    next_pending_tx,
  }))
}

pub(super) fn transaction_info(result: StateResult<Option<PoolTransactionInfo>>) -> RpcResult {
  let Some(info) = result.map_err(|error| state_error(error.into()))? else {
    return Ok(json!(TxWithPoolInfo::default()));
  };

  let (nonce, balance, balance_enough) = info.into_parts();

  Ok(json!(TxWithPoolInfo {
    exist: true,
    packed: false,
    local_nonce: nonce,
    state_nonce: nonce,
    local_balance: balance,
    state_balance: balance,
    local_balance_enough: balance_enough,
    state_balance_enough: balance_enough,
  }))
}

pub(super) fn account_transactions(
  result: Result<(AccountPoolTransactions, Network), CoreStateQueryError<cfx_statedb::Error>>,
) -> RpcResult {
  let (result, network) = result.map_err(error::state)?;

  let first_tx_status = result.first_status().map(|status| match status {
    PoolTransactionStatus::Pending => TransactionStatus::Ready,
    PoolTransactionStatus::Queued(QueuedReason::NonceGap { .. }) => {
      TransactionStatus::Pending(PendingReason::FutureNonce)
    }
    PoolTransactionStatus::Queued(QueuedReason::InsufficientBalance { .. }) => {
      TransactionStatus::Pending(PendingReason::NotEnoughCash)
    }
  });

  Ok(json!(AccountPendingTransactions {
    pending_transactions: result
      .transactions()
      .iter()
      .map(|transaction| pending_transaction(transaction, network))
      .collect(),
    first_tx_status,
    pending_count: result.total_count().into(),
  }))
}

pub(super) fn content(
  result: Result<(PoolContent, Network), CoreStateQueryError<cfx_statedb::Error>>,
  inspect: bool,
) -> RpcResult {
  let (content, network) = result.map_err(error::state)?;
  let (ready, queued) = content.into_parts();
  let project = |transaction: &RuntimeTransaction| {
    if inspect {
      json!(summary(transaction))
    } else {
      json!(pending_transaction(transaction, network))
    }
  };

  Ok(json!({
    "ready": grouped(ready.iter(), project),
    "deferred": grouped(ready.iter().chain(&queued), project),
  }))
}

pub(super) fn debug_account_transactions(
  result: Result<(PoolContent, Network), CoreStateQueryError<cfx_statedb::Error>>,
) -> RpcResult {
  let (content, network) = result.map_err(error::state)?;
  let (ready, queued) = content.into_parts();

  // The upstream debug operation concatenates ready and the complete deferred set.
  Ok(json!(
    ready
      .iter()
      .chain(&ready)
      .chain(&queued)
      .map(|transaction| pending_transaction(transaction, network))
      .collect::<Vec<_>>()
  ))
}

fn grouped<'a, T>(
  transactions: impl Iterator<Item = &'a RuntimeTransaction>,
  project: impl Fn(&RuntimeTransaction) -> T,
) -> BTreeMap<String, BTreeMap<String, Vec<T>>> {
  let mut grouped = BTreeMap::<String, BTreeMap<String, Vec<T>>>::new();

  for transaction in transactions {
    grouped
      .entry(format!("{:?}", transaction.sender_with_space()))
      .or_default()
      .entry(transaction.nonce().to_string())
      .or_default()
      .push(project(transaction));
  }

  grouped
}

fn summary(transaction: &RuntimeTransaction) -> String {
  let to = match transaction.transaction().action() {
    Action::Create => "<Create contract>".to_owned(),
    Action::Call(address) => format!("{address:?}"),
  };

  format!(
    "{to}: {:?} drip + {:?} gas * {:?} drip",
    transaction.value(),
    transaction.gas(),
    transaction.gas_price(),
  )
}
