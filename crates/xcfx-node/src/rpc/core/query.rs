//! Encodes completed Core history queries without borrowing the Runtime.

use std::sync::Arc;

use cfx_addr::Network;
use cfx_types::{H256, U256};
use serde_json::json;

use crate::{
  fork::ForkReadError,
  node::{
    CoreBlock, CoreBlockQueryError, CoreDebugTransactions, CoreEpochReceipts,
    CoreEpochReceiptsError, CoreReceipt, CoreTransaction, NativeNonceError,
  },
  rpc::{error::fork_error, operation::RpcResult, protocol::RpcError},
  runtime::CoreEpochQueryError,
};

use super::{convert, error};

pub(super) fn encode_debug_epoch(
  result: Result<(CoreDebugTransactions, Network), CoreEpochQueryError>,
) -> RpcResult {
  let (transactions, network) = result.map_err(error::epoch)?;
  Ok(json!(convert::debug_transactions(transactions, network)))
}

pub(super) fn encode_debug_block(
  result: Result<(Option<CoreDebugTransactions>, Network), ForkReadError>,
) -> RpcResult {
  let (transactions, network) = result.map_err(fork_error)?;
  let transactions = transactions.ok_or_else(RpcError::invalid_params)?;
  Ok(json!(convert::debug_transactions(transactions, network)))
}

pub(super) fn encode_block(
  result: Result<(Option<CoreBlock>, Network), CoreEpochQueryError>,
  full: bool,
) -> RpcResult {
  result
    .map(|(block, network)| json!(block.map(|block| convert::block(block, network, full))))
    .map_err(error::epoch)
}

pub(super) fn encode_block_with_pivot(
  result: Result<(CoreBlock, Network), CoreBlockQueryError>,
) -> RpcResult {
  result
    .map(|(block, network)| json!(convert::block(block, network, true)))
    .map_err(error::block)
}

pub(super) fn encode_transaction(
  result: Result<(Option<CoreTransaction>, Network), ForkReadError>,
) -> RpcResult {
  result
    .map(|(transaction, network)| json!(transaction.map(|tx| convert::transaction(tx, network))))
    .map_err(fork_error)
}

pub(super) fn encode_receipt(
  result: Result<(Option<CoreReceipt>, Network), ForkReadError>,
) -> RpcResult {
  result
    .map(|(receipt, network)| json!(receipt.map(|receipt| convert::receipt(receipt, network))))
    .map_err(fork_error)
}

pub(super) fn encode_epoch_receipts(
  result: Result<(Option<CoreEpochReceipts>, Network), CoreEpochReceiptsError>,
  include_espace: bool,
) -> RpcResult {
  result
    .map(|(receipts, network)| {
      json!(receipts.map(|receipts| convert::epoch_receipts(receipts, network, include_espace)))
    })
    .map_err(error::epoch_receipts)
}

pub(super) fn encode_epoch_blocks(result: Result<Arc<[H256]>, CoreEpochQueryError>) -> RpcResult {
  result
    .map(|hashes| json!(hashes.as_ref()))
    .map_err(error::epoch)
}

pub(super) fn encode_pending_nonce(result: Result<U256, NativeNonceError>) -> RpcResult {
  result
    .map(|nonce| json!(nonce))
    .map_err(error::pending_nonce)
}
