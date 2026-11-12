//! Core wire input normalization; instance-dependent checks remain on the worker.

use cfx_rpc_cfx_types::{
  BlockHashOrEpochNumber, EpochNumber, TransactionRequest as RpcTransactionRequest,
  access_list::to_primitive_access_list,
};
use primitives::Action;
use serde::{
  Deserialize,
  de::{Error as _, value::Error},
};

use crate::{
  runtime::{EpochSelector, StateSelector},
  transaction_request::{
    NativeTransactionRequest, NativeTransactionType, RequestSpace, TransactionRequest,
  },
};

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

pub(super) fn epoch_selector(epoch: EpochNumber) -> EpochSelector {
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

#[derive(Deserialize)]
#[serde(try_from = "RpcTransactionRequest")]
pub(super) struct CoreTransactionRequest(NativeTransactionRequest);

impl TryFrom<RpcTransactionRequest> for CoreTransactionRequest {
  type Error = Error;

  fn try_from(value: RpcTransactionRequest) -> Result<Self, Self::Error> {
    let mut network = None;

    for address in value.from.iter().chain(value.to.iter()) {
      if network.is_some_and(|network| network != address.network) {
        return Err(Error::custom(
          "Core transaction addresses use different networks",
        ));
      }
      network = Some(address.network);
    }

    let kind = value
      .transaction_type
      .map(|value| value.as_u64())
      .unwrap_or_else(|| {
        if value.max_fee_per_gas.is_some() || value.max_priority_fee_per_gas.is_some() {
          2
        } else if value.access_list.is_some() {
          1
        } else {
          0
        }
      });

    let transaction_type = match kind {
      0 => NativeTransactionType::Legacy,
      1 => NativeTransactionType::AccessList,
      2 => NativeTransactionType::DynamicFee,
      _ => return Err(Error::custom("unsupported Core transaction type")),
    };

    let request = TransactionRequest {
      from: value.from.map(|address| address.hex_address),
      nonce: value.nonce,
      gas: value.gas,
      action: value
        .to
        .map_or(Action::Create, |address| Action::Call(address.hex_address)),
      value: value.value.unwrap_or_default(),
      data: value.data.unwrap_or_default().into_vec(),
      // Core node signing and simulation derive these fields from the node and selected epoch.
      chain_id: None,
      gas_price: value.gas_price,
      max_fee_per_gas: value.max_fee_per_gas,
      max_priority_fee_per_gas: value.max_priority_fee_per_gas,
      access_list: value.access_list.map(to_primitive_access_list),
      space: RequestSpace::Native {
        transaction_type,
        storage_limit: value.storage_limit.map(|limit| limit.as_u64()),
        epoch_height: None,
      },
    };

    Ok(Self(NativeTransactionRequest::new(request, network)))
  }
}

impl From<CoreTransactionRequest> for NativeTransactionRequest {
  fn from(value: CoreTransactionRequest) -> Self {
    value.0
  }
}
