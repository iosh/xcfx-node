//! Assembly of protocol transactions from resolved fields.

use cfx_types::{AddressWithSpace, U256};
use primitives::{
  Action, Transaction,
  transaction::{
    Cip1559Transaction, Cip2930Transaction, Eip155Transaction, Eip1559Transaction,
    Eip2930Transaction, Eip7702Transaction, EthereumTransaction, NativeTransaction,
    TypedNativeTransaction,
  },
};

use super::{EthereumTransactionType, NativeTransactionType, TransactionRequest};

#[derive(Clone, Copy)]
pub(crate) enum TransactionType {
  Native(NativeTransactionType),
  Ethereum(EthereumTransactionType),
}

impl TransactionType {
  pub(crate) fn has_dynamic_fee(self) -> bool {
    matches!(
      self,
      Self::Native(NativeTransactionType::DynamicFee)
        | Self::Ethereum(
          EthereumTransactionType::DynamicFee | EthereumTransactionType::Authorization
        )
    )
  }
}

#[derive(Clone, Copy)]
pub(crate) struct TransactionFees {
  // A fixed price for legacy/access-list transactions, otherwise the fee cap.
  pub(crate) gas_price: U256,
  pub(crate) priority_fee: U256,
}

pub(crate) struct TransactionFields {
  pub(crate) sender: AddressWithSpace,
  pub(crate) nonce: U256,
  pub(crate) chain_id: u32,
  pub(crate) epoch_height: u64,
  pub(crate) fees: TransactionFees,
}

/// An immutable preparation result shared by submission estimation and final
/// assembly. Only gas and Core storage limits remain for the owning operation.
pub(crate) struct TransactionTemplate<'a> {
  request: &'a TransactionRequest,
  transaction_type: TransactionType,
  fields: TransactionFields,
}

impl<'a> TransactionTemplate<'a> {
  pub(crate) fn new(
    request: &'a TransactionRequest,
    transaction_type: TransactionType,
    fields: TransactionFields,
  ) -> Self {
    Self {
      request,
      transaction_type,
      fields,
    }
  }

  pub(crate) fn request(&self) -> &TransactionRequest {
    self.request
  }

  pub(crate) fn sender(&self) -> AddressWithSpace {
    self.fields.sender
  }

  pub(crate) fn nonce(&self) -> U256 {
    self.fields.nonce
  }

  pub(crate) fn build(&self, gas: U256, storage_limit: u64) -> Transaction {
    let TransactionFields {
      nonce,
      chain_id,
      epoch_height,
      fees,
      ..
    } = self.fields;
    let TransactionFees {
      gas_price,
      priority_fee,
    } = fees;
    let action = self.request.action;
    let value = self.request.value;
    let data = self.request.data.clone();
    let access_list = self.request.access_list.clone().unwrap_or_default();
    match self.transaction_type {
      TransactionType::Native(transaction_type) => Transaction::Native(match transaction_type {
        NativeTransactionType::Legacy => TypedNativeTransaction::Cip155(NativeTransaction {
          nonce,
          gas,
          gas_price,
          action,
          value,
          data,
          chain_id,
          epoch_height,
          storage_limit,
        }),
        NativeTransactionType::AccessList => TypedNativeTransaction::Cip2930(Cip2930Transaction {
          nonce,
          gas,
          gas_price,
          action,
          value,
          data,
          chain_id,
          epoch_height,
          storage_limit,
          access_list,
        }),
        NativeTransactionType::DynamicFee => TypedNativeTransaction::Cip1559(Cip1559Transaction {
          nonce,
          gas,
          max_fee_per_gas: gas_price,
          max_priority_fee_per_gas: priority_fee,
          action,
          value,
          data,
          chain_id,
          epoch_height,
          storage_limit,
          access_list,
        }),
      }),
      TransactionType::Ethereum(transaction_type) => {
        Transaction::Ethereum(match transaction_type {
          // The upstream Eip155 payload is the legacy RLP format; supplying a
          // chain ID enables its replay protection.
          EthereumTransactionType::Legacy => EthereumTransaction::Eip155(Eip155Transaction {
            nonce,
            gas,
            gas_price,
            action,
            value,
            data,
            chain_id: Some(chain_id),
          }),
          EthereumTransactionType::AccessList => EthereumTransaction::Eip2930(Eip2930Transaction {
            nonce,
            gas,
            gas_price,
            action,
            value,
            data,
            chain_id,
            access_list,
          }),
          EthereumTransactionType::DynamicFee => EthereumTransaction::Eip1559(Eip1559Transaction {
            nonce,
            gas,
            max_fee_per_gas: gas_price,
            max_priority_fee_per_gas: priority_fee,
            action,
            value,
            data,
            chain_id,
            access_list,
          }),
          EthereumTransactionType::Authorization => {
            let Action::Call(destination) = action else {
              unreachable!("request conversion requires a destination for authorization lists")
            };
            EthereumTransaction::Eip7702(Eip7702Transaction {
              nonce,
              gas,
              max_fee_per_gas: gas_price,
              max_priority_fee_per_gas: priority_fee,
              destination,
              value,
              data,
              chain_id,
              access_list,
              authorization_list: self
                .request
                .authorization_list()
                .cloned()
                .unwrap_or_default(),
            })
          }
        })
      }
    }
  }
}
