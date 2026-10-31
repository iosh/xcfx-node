//! Wire projections of pool results captured by one Worker operation.

use std::collections::BTreeMap;

use cfx_rpc_cfx_types::{PendingReason, TransactionStatus};
use cfx_rpc_eth_types::{
  AccountPendingTransactions, TxpoolContent, TxpoolContentFrom, TxpoolStatus,
};
use cfx_statedb::Result as StateResult;
use cfx_types::{Address, U64};
use primitives::Action;
use serde_json::json;

use crate::{
  rpc::operation::RpcResult,
  runtime::{
    AccountPoolTransactions, PoolContent, PoolTransactionCounts, PoolTransactionStatus,
    QueuedReason,
  },
  runtime_transaction::RuntimeTransaction,
};

use super::{convert::pending_transaction, error::state_error};

pub(super) fn status(result: StateResult<PoolTransactionCounts>) -> RpcResult {
  let counts = result.map_err(|error| state_error(error.into()))?;
  Ok(json!(TxpoolStatus {
    pending: U64::from(counts.pending()),
    queued: U64::from(counts.queued()),
  }))
}

pub(super) fn content(result: StateResult<PoolContent>) -> RpcResult {
  let (pending, queued) = result
    .map_err(|error| state_error(error.into()))?
    .into_parts();
  Ok(json!(TxpoolContent {
    pending: by_account(pending, pending_transaction),
    queued: by_account(queued, pending_transaction),
  }))
}

pub(super) fn content_from(result: StateResult<PoolContent>) -> RpcResult {
  let (pending, queued) = result
    .map_err(|error| state_error(error.into()))?
    .into_parts();
  Ok(json!(TxpoolContentFrom {
    pending: by_nonce(pending),
    queued: by_nonce(queued),
  }))
}

pub(super) fn inspect(result: StateResult<PoolContent>) -> RpcResult {
  let (pending, queued) = result
    .map_err(|error| state_error(error.into()))?
    .into_parts();
  Ok(json!(TxpoolContent {
    pending: by_account(pending, summary),
    queued: by_account(queued, summary),
  }))
}

pub(super) fn account_transactions(result: StateResult<AccountPoolTransactions>) -> RpcResult {
  let result = result.map_err(|error| state_error(error.into()))?;
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
      .map(pending_transaction)
      .collect(),
    first_tx_status,
    pending_count: U64::from(result.total_count()),
  }))
}

fn by_account<T>(
  transactions: Vec<RuntimeTransaction>,
  project: impl Fn(&RuntimeTransaction) -> T,
) -> BTreeMap<Address, BTreeMap<String, T>> {
  let mut accounts = BTreeMap::<Address, BTreeMap<String, T>>::new();
  for transaction in transactions {
    accounts
      .entry(transaction.sender())
      .or_default()
      .insert(transaction.nonce().to_string(), project(&transaction));
  }
  accounts
}

fn by_nonce(
  transactions: Vec<RuntimeTransaction>,
) -> BTreeMap<String, cfx_rpc_eth_types::Transaction> {
  transactions
    .into_iter()
    .map(|transaction| {
      (
        transaction.nonce().to_string(),
        pending_transaction(&transaction),
      )
    })
    .collect()
}

fn summary(transaction: &RuntimeTransaction) -> String {
  let to = match transaction.transaction().action() {
    Action::Create => "contract creation".to_owned(),
    Action::Call(address) => format!("{address:?}"),
  };
  // Preserve the full transaction range; the upstream summary narrows these fields.
  format!(
    "{to}: {} wei + {} gas × {} wei",
    transaction.value(),
    transaction.gas(),
    transaction.gas_price(),
  )
}
