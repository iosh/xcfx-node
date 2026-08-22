use cfx_executor::{
  spec::TransitionsEpochHeight,
  transaction_validation::{
    LocalValidationMode, ValidationContext, ValidationMode, validate_transaction_common,
  },
};
use cfx_types::{AllChainID, U256};
use cfx_vm_types::Spec;
use primitives::{
  TransactionWithSignature,
  block::BlockHeight,
  transaction::{SignedTransaction, TransactionError},
};

/// Protocol inputs required to process a raw transaction at one chain view.
pub(crate) struct TransactionIngressContext<'a> {
  pub(crate) chain_id: AllChainID,
  pub(crate) height: BlockHeight,
  pub(crate) transitions: &'a TransitionsEpochHeight,
  pub(crate) transaction_epoch_bound: u64,
  pub(crate) max_nonce: Option<U256>,
  pub(crate) spec: &'a Spec,
}

/// Decode, validate, and recover the sender of a raw transaction.
pub(crate) fn decode_and_validate_raw_transaction(
  raw: &[u8],
  context: &TransactionIngressContext<'_>,
) -> Result<SignedTransaction, TransactionError> {
  let tx = TransactionWithSignature::from_raw(raw)?;

  validate_transaction_common(
    &tx,
    &ValidationContext {
      chain_id: context.chain_id,
      height: context.height,
      transitions: context.transitions,
      transaction_epoch_bound: context.transaction_epoch_bound,
      max_nonce: context.max_nonce,
      mode: ValidationMode::Local(LocalValidationMode::Full, context.spec),
    },
  )?;
  let public = tx.recover_public()?;
  Ok(SignedTransaction::new(public, tx))
}
