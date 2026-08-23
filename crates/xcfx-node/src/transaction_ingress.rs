use cfx_executor::{
  spec::TransitionsEpochHeight,
  transaction_validation::{
    LocalValidationMode, ValidationContext as ExecutorValidationContext, ValidationMode,
    validate_transaction_common,
  },
};
use cfx_types::{AllChainID, U256};
use cfx_vm_types::Spec;
use primitives::{
  TransactionWithSignature,
  block::BlockHeight,
  transaction::{SignedTransaction, TransactionError},
};

/// Protocol inputs required to validate a transaction at one chain view.
pub(crate) struct TransactionValidationContext<'a> {
  pub(crate) chain_id: AllChainID,
  pub(crate) height: BlockHeight,
  pub(crate) transitions: &'a TransitionsEpochHeight,
  pub(crate) transaction_epoch_bound: u64,
  pub(crate) max_nonce: Option<U256>,
  pub(crate) spec: &'a Spec,
}

impl TransactionValidationContext<'_> {
  pub(crate) fn validate(
    &self,
    transaction: &TransactionWithSignature,
  ) -> Result<(), TransactionError> {
    validate_transaction_common(
      transaction,
      &ExecutorValidationContext {
        chain_id: self.chain_id,
        height: self.height,
        transitions: self.transitions,
        transaction_epoch_bound: self.transaction_epoch_bound,
        max_nonce: self.max_nonce,
        mode: ValidationMode::Local(LocalValidationMode::Full, self.spec),
      },
    )
  }
}

/// Decode, validate, and recover the sender of a raw transaction.
pub(crate) fn decode_and_validate_raw_transaction(
  raw: &[u8],
  context: &TransactionValidationContext<'_>,
) -> Result<SignedTransaction, TransactionError> {
  let transaction = TransactionWithSignature::from_raw(raw)?;
  context.validate(&transaction)?;
  let public = transaction.recover_public()?;

  Ok(SignedTransaction::new(public, transaction))
}
