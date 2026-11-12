//! Public Native state facts, independent of RPC encoding and execution ownership.

use cfx_types::{Address, H256, U256};

#[derive(Clone)]
pub(crate) struct AccountInfo {
  pub(crate) address: Address,
  pub(crate) balance: U256,
  pub(crate) nonce: U256,
  pub(crate) code_hash: H256,
  pub(crate) staking_balance: U256,
  /// Token collateral plus the drip value of used storage points.
  pub(crate) collateral_for_storage: U256,
  pub(crate) accumulated_interest_return: U256,
  pub(crate) admin: Address,
}

#[derive(Clone)]
pub(crate) struct SponsorInfo {
  pub(crate) sponsor_for_gas: Address,
  pub(crate) sponsor_for_collateral: Address,
  pub(crate) sponsor_gas_bound: U256,
  pub(crate) sponsor_balance_for_gas: U256,
  pub(crate) sponsor_balance_for_collateral: U256,
  /// Public storage-point quantities are counts of bytes, rounded down.
  pub(crate) available_storage_points: U256,
  pub(crate) used_storage_points: U256,
}

#[derive(Clone)]
pub(crate) struct PosEconomics {
  pub(crate) total_pos_staking_tokens: U256,
  pub(crate) distributable_pos_interest: U256,
  pub(crate) last_distribute_block: u64,
}

#[derive(Clone)]
pub(crate) struct TokenSupplyInfo {
  pub(crate) total_circulating: U256,
  pub(crate) total_issued: U256,
  pub(crate) total_staking: U256,
  pub(crate) total_collateral: U256,
  pub(crate) total_espace_tokens: U256,
}

#[derive(Clone)]
pub(crate) struct StorageCollateralInfo {
  pub(crate) total_storage_tokens: U256,
  /// Public storage-point quantities are counts of bytes, rounded down.
  pub(crate) converted_storage_points: U256,
  pub(crate) used_storage_points: U256,
}

#[derive(Clone)]
pub(crate) struct VoteParams {
  pub(crate) pow_base_reward: U256,
  /// Interest per block, unlike the annualized value stored in state.
  pub(crate) interest_rate: U256,
  pub(crate) storage_point_prop: U256,
  pub(crate) base_fee_share_prop: U256,
}

/// Declared costs and participants for a read-only sponsorship and balance check.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct BalanceCheckRequest {
  pub(crate) account: Address,
  pub(crate) contract: Address,
  pub(crate) gas_limit: U256,
  pub(crate) gas_price: U256,
  pub(crate) storage_limit: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct BalanceCheckResult {
  pub(crate) will_pay_tx_fee: bool,
  pub(crate) will_pay_collateral: bool,
  pub(crate) is_balance_enough: bool,
}
