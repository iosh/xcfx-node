use std::collections::VecDeque;

use cfx_rpc_cfx_types::{CfxFeeHistory, EpochNumber};
use cfx_types::U64;
use serde_json::{Value, json};

use crate::{
  fee_history::{FeeHistory, RewardPercentiles},
  rpc::{error::fee_query_error, operation::Operation, protocol::RpcError},
};

pub(super) fn history(count: U64, newest: EpochNumber, percentiles: Option<Vec<f64>>) -> Operation {
  if newest == EpochNumber::LatestMined {
    return Operation::Immediate(Err(RpcError::invalid_params()));
  }

  if count.is_zero() {
    return Operation::Immediate(Ok(history_value(FeeHistory::default())));
  }

  let percentiles = match RewardPercentiles::new(percentiles.unwrap_or_default()) {
    Ok(percentiles) => percentiles,
    Err(_) => return Operation::Immediate(Err(RpcError::invalid_params())),
  };

  let selector = super::params::epoch_selector(newest);

  Operation::on_node(
    move |node| node.core_fee_history(count.as_u64(), selector, percentiles),
    |result| result.map(history_value).map_err(fee_query_error),
  )
}

fn history_value(history: FeeHistory) -> Value {
  let Some((range, next_base_fee)) = history.into_parts() else {
    return json!(CfxFeeHistory::default());
  };

  let (oldest_height, blocks) = range.into_parts();
  let mut base_fees = VecDeque::new();
  let mut gas_ratios = VecDeque::new();
  let mut rewards = VecDeque::new();

  for block in blocks {
    let (base_fee, gas_ratio, reward) = block.into_parts();
    base_fees.push_back(base_fee);
    gas_ratios.push_back(gas_ratio);
    rewards.push_back(reward);
  }

  base_fees.push_back(next_base_fee);

  json!(CfxFeeHistory::new(
    oldest_height.into(),
    base_fees,
    gas_ratios,
    rewards
  ))
}
