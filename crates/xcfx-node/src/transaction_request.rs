//! Normalized transaction input, before an operation selects defaults or fees.

pub(crate) mod build;

use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, Space, U256};
use primitives::{AccessList, Action, AuthorizationList};

/// A normalized Native request and its sender/destination address network.
/// The RPC adapter rejects inconsistent networks before constructing it.
pub(crate) struct NativeTransactionRequest {
  request: TransactionRequest,
  network: Option<cfx_addr::Network>,
}

impl NativeTransactionRequest {
  pub(crate) fn new(request: TransactionRequest, network: Option<cfx_addr::Network>) -> Self {
    assert_eq!(
      request.space(),
      Space::Native,
      "a Native request must carry Native transaction fields"
    );
    Self { request, network }
  }

  pub(crate) fn into_parts(self) -> (TransactionRequest, Option<cfx_addr::Network>) {
    (self.request, self.network)
  }
}

#[derive(Clone, Copy)]
pub(crate) enum NativeTransactionType {
  Legacy,
  AccessList,
  DynamicFee,
}

#[derive(Clone, Copy)]
pub(crate) enum EthereumTransactionType {
  Legacy,
  AccessList,
  DynamicFee,
  Authorization,
}

#[derive(Clone)]
pub(crate) enum RequestSpace {
  Native {
    transaction_type: NativeTransactionType,
    storage_limit: Option<u64>,
    epoch_height: Option<u64>,
  },
  Ethereum {
    authorization_list: Option<AuthorizationList>,
  },
}

/// Field presence is part of the input: an omitted fee, gas limit or list is
/// different from an explicit zero or empty list. Preparation borrows this value.
#[derive(Clone)]
pub(crate) struct TransactionRequest {
  pub(crate) from: Option<Address>,
  pub(crate) nonce: Option<U256>,
  pub(crate) gas: Option<U256>,
  pub(crate) action: Action,
  pub(crate) value: U256,
  pub(crate) data: Vec<u8>,
  pub(crate) chain_id: Option<u32>,
  pub(crate) gas_price: Option<U256>,
  pub(crate) max_fee_per_gas: Option<U256>,
  pub(crate) max_priority_fee_per_gas: Option<U256>,
  pub(crate) access_list: Option<AccessList>,
  pub(crate) space: RequestSpace,
}

impl TransactionRequest {
  pub(crate) fn space(&self) -> Space {
    match self.space {
      RequestSpace::Native { .. } => Space::Native,
      RequestSpace::Ethereum { .. } => Space::Ethereum,
    }
  }

  pub(crate) fn from(&self) -> Option<AddressWithSpace> {
    self.from.map(|address| address.with_space(self.space()))
  }

  pub(crate) fn has_dynamic_fees(&self) -> bool {
    self.max_fee_per_gas.is_some() || self.max_priority_fee_per_gas.is_some()
  }

  pub(crate) fn authorization_list(&self) -> Option<&AuthorizationList> {
    match &self.space {
      RequestSpace::Ethereum { authorization_list } => authorization_list.as_ref(),
      RequestSpace::Native { .. } => None,
    }
  }

  pub(crate) fn storage_limit(&self) -> Option<u64> {
    match self.space {
      RequestSpace::Native { storage_limit, .. } => storage_limit,
      RequestSpace::Ethereum { .. } => None,
    }
  }
}
