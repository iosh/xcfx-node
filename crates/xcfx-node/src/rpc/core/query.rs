//! Core position adaptation and encoding of completed field reads.

use std::sync::Arc;

use cfx_rpc_cfx_types::{
  BlockHashOrEpochNumber, Bytes, EpochNumber, address::UnexpectedRpcAddressNetwork,
};
use cfx_types::U256;
use serde::Serialize;
use serde_json::json;

use crate::{
  node::{CoreStateQueryError, NativeNonceError},
  rpc::{
    error::state_error,
    operation::{Operation, RpcResult},
    protocol::RpcError,
  },
  runtime::{EpochSelector, StateSelector},
};

pub(super) fn epoch_number(epoch: Option<EpochNumber>) -> Operation {
  let selector = epoch_selector(epoch.unwrap_or(EpochNumber::LatestMined));
  Operation::on_node(
    move |node| node.core_epoch_number(selector),
    |height| {
      height
        .map(|height| json!(U256::from(height)))
        .ok_or_else(RpcError::invalid_params)
    },
  )
}

pub(super) fn state_selector(selector: Option<BlockHashOrEpochNumber>) -> StateSelector {
  match selector {
    Some(BlockHashOrEpochNumber::BlockHashWithOption {
      hash,
      require_pivot,
    }) => StateSelector::Hash {
      hash,
      require_pivot: require_pivot.unwrap_or(true),
    },
    Some(BlockHashOrEpochNumber::EpochNumber(epoch)) => StateSelector::Epoch(epoch_selector(epoch)),
    None => StateSelector::Epoch(EpochSelector::LatestState),
  }
}

fn epoch_selector(epoch: EpochNumber) -> EpochSelector {
  match epoch {
    EpochNumber::Num(height) => EpochSelector::Number(height.as_u64()),
    EpochNumber::Earliest => EpochSelector::Earliest,
    EpochNumber::LatestMined => EpochSelector::LatestMined,
    EpochNumber::LatestState => EpochSelector::LatestState,
    EpochNumber::LatestConfirmed => EpochSelector::Confirmed,
    EpochNumber::LatestFinalized => EpochSelector::Finalized,
    EpochNumber::LatestCheckpoint => EpochSelector::Checkpoint,
  }
}

pub(super) fn state<T: Serialize>(result: Result<T, CoreStateQueryError>) -> RpcResult {
  result.map(|value| json!(value)).map_err(query_error)
}

pub(super) fn code(result: Result<Arc<Vec<u8>>, CoreStateQueryError>) -> RpcResult {
  result
    .map(|code| json!(Bytes::new(Arc::unwrap_or_clone(code))))
    .map_err(query_error)
}

pub(super) fn pending_nonce(result: Result<U256, NativeNonceError>) -> RpcResult {
  result
    .map(|nonce| json!(nonce))
    .map_err(|error| match error {
      NativeNonceError::Network(error) => network_error(error),
      NativeNonceError::State(error) => state_error(error.into()),
    })
}

fn query_error(error: CoreStateQueryError) -> RpcError {
  match error {
    CoreStateQueryError::Network(error) => network_error(error),
    CoreStateQueryError::Query(error) => state_error(error),
  }
}

fn network_error(error: UnexpectedRpcAddressNetwork) -> RpcError {
  RpcError {
    code: -32602,
    message: error.to_string(),
    data: None,
  }
}
