//! Fee-history request adaptation and result encoding.

use cfx_rpc_eth_types::BlockId;
use cfx_types::{U64, U256};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
  fee_history::{FeeHistory, RewardPercentiles},
  rpc::{
    operation::{Operation, RpcResult},
    protocol::RpcError,
  },
  runtime::{BlockPreparationError, FeeQueryError},
};

use super::{
  block_selector,
  error::{fork_error, invalid_params, state_error},
};

pub(super) fn history(
  block_count: U64,
  newest_block: BlockId,
  percentiles: Option<Vec<f64>>,
) -> Operation {
  // Conflux handles these empty requests before percentile validation or position lookup.
  if block_count.is_zero() || newest_block == BlockId::Pending {
    return Operation::Immediate(Ok(history_value(FeeHistory::default())));
  }
  let percentiles = match RewardPercentiles::new(percentiles.unwrap_or_default()) {
    Ok(percentiles) => percentiles,
    Err(error) => return Operation::Immediate(Err(invalid_params(error))),
  };
  let selector = block_selector(newest_block);
  Operation::on_node(
    move |node| node.espace_fee_history(block_count.as_u64(), selector, percentiles),
    |result| result.map(history_value).map_err(query_error),
  )
}

pub(super) fn priority_fee(result: Result<U256, FeeQueryError>) -> RpcResult {
  Ok(json!(result.map_err(query_error)?))
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeeHistoryResponse {
  oldest_block: U64,
  base_fee_per_gas: Vec<U256>,
  gas_used_ratio: Vec<f64>,
  reward: Vec<Vec<U256>>,
}

fn history_value(history: FeeHistory) -> Value {
  let mut response = FeeHistoryResponse::default();
  if let Some((range, next_base_fee)) = history.into_parts() {
    let (oldest_height, blocks) = range.into_parts();
    response.oldest_block = oldest_height.into();
    for block in blocks {
      let (base_fee, gas_used_ratio, rewards) = block.into_parts();
      response.base_fee_per_gas.push(base_fee);
      response.gas_used_ratio.push(gas_used_ratio);
      response.reward.push(rewards);
    }
    response.base_fee_per_gas.push(next_base_fee);
  }
  json!(response)
}

fn query_error(error: FeeQueryError) -> RpcError {
  match error {
    FeeQueryError::UnknownBlock => RpcError::invalid_params(),
    FeeQueryError::Fork(error) => fork_error(error),
    FeeQueryError::Preparation(BlockPreparationError::State(error)) => state_error(error.into()),
    error @ (FeeQueryError::Preparation(BlockPreparationError::Time(_))
    | FeeQueryError::Preparation(BlockPreparationError::PositionExhausted(_))
    | FeeQueryError::SuggestionOverflow) => RpcError {
      code: -32000,
      message: error.to_string(),
      data: None,
    },
  }
}
