//! eSpace wire data is normalized here before an operation reaches the worker.

use std::collections::BTreeMap;

use cfx_rpc_eth_types::{RpcStateOverride, TransactionRequest as RpcTransactionRequest};
use cfx_types::{Address, H256, U64, U256};
use primitives::Action;
use serde::{
  Deserialize,
  de::{Error as _, value::Error},
};

use crate::{
  transaction_request::{RequestSpace, TransactionRequest},
  virtual_execution::{
    VirtualAccountOverride, VirtualEnvironmentOverrides, VirtualStateOverrides,
    VirtualStorageOverride,
  },
};

/// The endpoint fixes the Space; omitted fields remain for the operation to resolve.
#[derive(Deserialize)]
#[serde(try_from = "RpcTransactionRequest")]
pub(super) struct EspaceTransactionRequest(TransactionRequest);

impl TryFrom<RpcTransactionRequest> for EspaceTransactionRequest {
  type Error = Error;

  fn try_from(request: RpcTransactionRequest) -> Result<Self, Self::Error> {
    if request.authorization_list.is_some() && request.to.is_none() {
      return Err(Error::custom(
        "to is required for an authorization transaction",
      ));
    }
    let data = request
      .input
      .try_into_unique_input()
      .map_err(Error::custom)?
      .unwrap_or_default()
      .to_vec();
    let chain_id = request
      .chain_id
      .map(u32::try_from)
      .transpose()
      .map_err(|error| Error::custom(format!("invalid chainId: {error}")))?;
    Ok(Self(TransactionRequest {
      from: request.from,
      nonce: request.nonce,
      gas: request.gas,
      action: request.to.map_or(Action::Create, Action::Call),
      value: request.value.unwrap_or_default(),
      data,
      chain_id,
      gas_price: request.gas_price,
      max_fee_per_gas: request.max_fee_per_gas,
      max_priority_fee_per_gas: request.max_priority_fee_per_gas,
      access_list: request.access_list,
      space: RequestSpace::Ethereum {
        authorization_list: request
          .authorization_list
          .map(|items| items.into_iter().map(Into::into).collect()),
      },
    }))
  }
}

impl From<EspaceTransactionRequest> for TransactionRequest {
  fn from(request: EspaceTransactionRequest) -> Self {
    request.0
  }
}

#[derive(Default, Deserialize)]
#[serde(try_from = "RpcStateOverride")]
pub(super) struct StateOverrides(VirtualStateOverrides);

impl TryFrom<RpcStateOverride> for StateOverrides {
  type Error = Error;

  fn try_from(state: RpcStateOverride) -> Result<Self, Self::Error> {
    let mut accounts = BTreeMap::new();
    for (address, account) in state {
      if account.move_precompile_to.is_some() {
        return Err(Error::custom("moving precompiles is not supported"));
      }
      let storage = match (account.state, account.state_diff) {
        (None, None) => None,
        (Some(storage), None) => Some(VirtualStorageOverride::Replace(
          storage.into_iter().collect(),
        )),
        (None, Some(storage)) => Some(VirtualStorageOverride::Diff(storage.into_iter().collect())),
        (Some(_), Some(_)) => {
          return Err(Error::custom("state and stateDiff are mutually exclusive"));
        }
      };
      accounts.insert(
        address,
        VirtualAccountOverride {
          balance: account.balance,
          nonce: account.nonce.map(|value| value.as_u64()),
          code: account.code.map(|value| value.0),
          storage,
        },
      );
    }
    Ok(Self(VirtualStateOverrides::new(accounts)))
  }
}

impl From<StateOverrides> for VirtualStateOverrides {
  fn from(overrides: StateOverrides) -> Self {
    overrides.0
  }
}

#[derive(Default, Deserialize)]
#[serde(try_from = "RpcBlockOverrides")]
pub(super) struct BlockOverrides(VirtualEnvironmentOverrides);

#[derive(Deserialize)]
#[serde(untagged)]
enum QuantityOrNumber {
  Quantity(U64),
  Number(u64),
}

impl QuantityOrNumber {
  fn into_u64(self) -> u64 {
    match self {
      Self::Quantity(value) => value.as_u64(),
      Self::Number(value) => value,
    }
  }
}

// The upstream block DTO uses plain u64; this endpoint also accepts hex quantities.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct RpcBlockOverrides {
  #[serde(alias = "blockNumber")]
  number: Option<QuantityOrNumber>,
  difficulty: Option<U256>,
  #[serde(alias = "timestamp")]
  time: Option<QuantityOrNumber>,
  gas_limit: Option<QuantityOrNumber>,
  #[serde(alias = "feeRecipient")]
  coinbase: Option<Address>,
  #[serde(alias = "prevRandao")]
  random: Option<H256>,
  #[serde(alias = "baseFeePerGas")]
  base_fee: Option<U256>,
  block_hash: Option<BTreeMap<u64, H256>>,
}

impl TryFrom<RpcBlockOverrides> for BlockOverrides {
  type Error = Error;

  fn try_from(value: RpcBlockOverrides) -> Result<Self, Self::Error> {
    if value.base_fee.is_some() || value.random.is_some() || value.block_hash.is_some() {
      return Err(Error::custom(
        "base fee, prevRandao and block hashes are not supported",
      ));
    }
    Ok(Self(VirtualEnvironmentOverrides {
      number: value.number.map(QuantityOrNumber::into_u64),
      difficulty: value.difficulty,
      timestamp: value.time.map(QuantityOrNumber::into_u64),
      gas_limit: value.gas_limit.map(|value| U256::from(value.into_u64())),
      author: value.coinbase,
    }))
  }
}

impl From<BlockOverrides> for VirtualEnvironmentOverrides {
  fn from(overrides: BlockOverrides) -> Self {
    overrides.0
  }
}
