//! Adapts protocol validation to product-owned virtual transaction payloads.

use cfx_executor::transaction_validation::{
  LocalValidationMode, ValidationContext, ValidationMode, validate_transaction_common,
};
use cfx_parameters::consensus::TRANSACTION_DEFAULT_EPOCH_BOUND;
use cfx_types::{AllChainID, U256};
use primitives::{
  SignedTransaction, Transaction,
  transaction::{EthereumTransaction, TransactionError},
};

use crate::runtime_transaction::fake_sign_for_execution;

use super::SimulationContext;

impl SimulationContext<'_> {
  pub(super) fn validate(&self, transaction: &SignedTransaction) -> Result<(), TransactionError> {
    // Common admission validation forbids zero prices before checking gas,
    // activation and payload rules. A priced validation copy lets all of those
    // checks run; execution retains the original zero-price payload. A zero
    // cap with positive priority is invalid and must not use this adaptation.
    let priced = match &transaction.unsigned {
      Transaction::Ethereum(payload)
        if payload.gas_price().is_zero()
          && transaction.unsigned.max_priority_gas_price().is_zero() =>
      {
        let mut payload = payload.clone();
        match &mut payload {
          EthereumTransaction::Eip155(tx) => tx.gas_price = U256::one(),
          EthereumTransaction::Eip2930(tx) => tx.gas_price = U256::one(),
          EthereumTransaction::Eip1559(tx) => tx.max_fee_per_gas = U256::one(),
          EthereumTransaction::Eip7702(tx) => tx.max_fee_per_gas = U256::one(),
        }
        Some(fake_sign_for_execution(
          Transaction::Ethereum(payload),
          transaction.sender().address,
        ))
      }
      _ => None,
    };
    let validated = priced.as_ref().unwrap_or(transaction);
    validate_transaction_common(
      validated,
      &ValidationContext {
        // CHAINID remains the selected environment's value during execution.
        chain_id: AllChainID::fake_for_virtual(transaction.chain_id().unwrap_or(1)),
        height: self.env.epoch_height,
        transitions: &self.machine.params().transition_heights,
        transaction_epoch_bound: TRANSACTION_DEFAULT_EPOCH_BOUND,
        max_nonce: Some(U256::from(u64::MAX - 1)),
        mode: ValidationMode::Local(LocalValidationMode::Full, &self.spec),
      },
    )
  }
}
