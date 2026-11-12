//! Encodes completed Core state reads without borrowing the Runtime.

use std::sync::Arc;

use cfx_addr::Network;
use cfx_rpc_cfx_types::{
  Account, Bytes, CheckBalanceAgainstTransactionResponse, PoSEconomics, SponsorInfo,
  StorageCollateralInfo, TokenSupplyInfo, VoteParamsInfo,
};
use cfx_types::Address;
use serde::Serialize;
use serde_json::json;

use crate::{
  native_state,
  node::CoreStateQueryError,
  rpc::{error::state_error, operation::RpcResult},
  runtime::StateQueryError,
};

use super::{convert, error};

pub(super) fn encode_result<T: Serialize>(result: Result<T, CoreStateQueryError>) -> RpcResult {
  result.map(|value| json!(value)).map_err(error::state)
}

pub(super) fn encode_global_result<T: Serialize>(result: Result<T, StateQueryError>) -> RpcResult {
  result.map(|value| json!(value)).map_err(state_error)
}

pub(super) fn encode_code(result: Result<Arc<Vec<u8>>, CoreStateQueryError>) -> RpcResult {
  result
    .map(|code| json!(Bytes::new(Arc::unwrap_or_clone(code))))
    .map_err(error::state)
}

pub(super) fn encode_admin(
  result: Result<(Option<Address>, Network), CoreStateQueryError>,
) -> RpcResult {
  let (admin, network) = result.map_err(error::state)?;
  Ok(json!(
    admin.map(|address| convert::address(address, network))
  ))
}

pub(super) fn encode_account(
  result: Result<(native_state::AccountInfo, Network), CoreStateQueryError>,
) -> RpcResult {
  let (account, network) = result.map_err(error::state)?;

  Ok(json!(Account {
    address: convert::address(account.address, network),
    balance: account.balance,
    nonce: account.nonce,
    code_hash: account.code_hash,
    staking_balance: account.staking_balance,
    collateral_for_storage: account.collateral_for_storage,
    accumulated_interest_return: account.accumulated_interest_return,
    admin: convert::address(account.admin, network),
  }))
}

pub(super) fn encode_sponsor_info(
  result: Result<(native_state::SponsorInfo, Network), CoreStateQueryError>,
) -> RpcResult {
  let (sponsor, network) = result.map_err(error::state)?;

  Ok(json!(SponsorInfo {
    sponsor_for_gas: convert::address(sponsor.sponsor_for_gas, network),
    sponsor_for_collateral: convert::address(sponsor.sponsor_for_collateral, network),
    sponsor_gas_bound: sponsor.sponsor_gas_bound,
    sponsor_balance_for_gas: sponsor.sponsor_balance_for_gas,
    sponsor_balance_for_collateral: sponsor.sponsor_balance_for_collateral,
    available_storage_points: sponsor.available_storage_points,
    used_storage_points: sponsor.used_storage_points,
  }))
}

pub(super) fn encode_pos_economics(
  result: Result<native_state::PosEconomics, StateQueryError>,
) -> RpcResult {
  let economics = result.map_err(state_error)?;

  Ok(json!(PoSEconomics {
    total_pos_staking_tokens: economics.total_pos_staking_tokens,
    distributable_pos_interest: economics.distributable_pos_interest,
    last_distribute_block: economics.last_distribute_block.into(),
  }))
}

pub(super) fn encode_supply_info(
  result: Result<native_state::TokenSupplyInfo, StateQueryError>,
) -> RpcResult {
  let supply = result.map_err(state_error)?;

  Ok(json!(TokenSupplyInfo {
    total_circulating: supply.total_circulating,
    total_issued: supply.total_issued,
    total_staking: supply.total_staking,
    total_collateral: supply.total_collateral,
    total_espace_tokens: supply.total_espace_tokens,
  }))
}

pub(super) fn encode_collateral_info(
  result: Result<native_state::StorageCollateralInfo, StateQueryError>,
) -> RpcResult {
  let collateral = result.map_err(state_error)?;

  Ok(json!(StorageCollateralInfo {
    total_storage_tokens: collateral.total_storage_tokens,
    converted_storage_points: collateral.converted_storage_points,
    used_storage_points: collateral.used_storage_points,
  }))
}

pub(super) fn encode_vote_params(
  result: Result<native_state::VoteParams, StateQueryError>,
) -> RpcResult {
  let params = result.map_err(state_error)?;

  Ok(json!(VoteParamsInfo {
    pow_base_reward: params.pow_base_reward,
    interest_rate: params.interest_rate,
    storage_point_prop: params.storage_point_prop,
    base_fee_share_prop: params.base_fee_share_prop,
  }))
}

pub(super) fn encode_balance_check(
  result: Result<native_state::BalanceCheckResult, CoreStateQueryError>,
) -> RpcResult {
  let check = result.map_err(error::state)?;

  Ok(json!(CheckBalanceAgainstTransactionResponse {
    will_pay_tx_fee: check.will_pay_tx_fee,
    will_pay_collateral: check.will_pay_collateral,
    is_balance_enough: check.is_balance_enough,
  }))
}
