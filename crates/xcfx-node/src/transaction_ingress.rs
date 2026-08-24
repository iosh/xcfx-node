use cfx_executor::{
  spec::TransitionsEpochHeight,
  transaction_validation::{
    LocalValidationMode, PackingCheckResult, ValidationContext as ExecutorValidationContext,
    ValidationMode, check_transaction_for_packing, validate_transaction_common,
  },
};
use cfx_parameters::block::MAX_BLOCK_SIZE_IN_BYTES;
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
  fn validate_with_mode(
    &self,
    transaction: &TransactionWithSignature,
    mode: LocalValidationMode,
  ) -> Result<(), TransactionError> {
    validate_transaction_common(
      transaction,
      &ExecutorValidationContext {
        chain_id: self.chain_id,
        height: self.height,
        transitions: self.transitions,
        transaction_epoch_bound: self.transaction_epoch_bound,
        max_nonce: self.max_nonce,
        mode: ValidationMode::Local(mode, self.spec),
      },
    )
  }

  pub(crate) fn validate_for_pool_admission(
    &self,
    transaction: &TransactionWithSignature,
  ) -> Result<(), TransactionError> {
    self.validate_with_mode(transaction, LocalValidationMode::MaybeLater)
  }

  pub(crate) fn validate_for_packing(
    &self,
    transaction: &TransactionWithSignature,
  ) -> Result<(), TransactionError> {
    self.validate_with_mode(transaction, LocalValidationMode::Full)
  }

  pub(crate) fn check_for_packing(
    &self,
    transaction: &TransactionWithSignature,
  ) -> PackingCheckResult {
    let fast_result = check_transaction_for_packing(
      transaction,
      self.height,
      self.transitions,
      self.transaction_epoch_bound,
      self.spec,
    );

    if !matches!(fast_result, PackingCheckResult::Pack) {
      return fast_result;
    }

    // The shared fast check covers height-gated packing rules. Complete the
    // common validation here so a newly activated rule cannot leave a pool
    // head permanently blocking its nonce successors.
    match self.validate_for_packing(transaction) {
      Ok(()) => PackingCheckResult::Pack,
      Err(_) => match self.validate_for_pool_admission(transaction) {
        Ok(()) => PackingCheckResult::Pending,
        Err(_) => PackingCheckResult::Drop,
      },
    }
  }
}

/// Decode, validate, and recover the sender of a raw transaction.
pub(crate) fn decode_and_validate_raw_transaction(
  raw: &[u8],
  context: &TransactionValidationContext<'_>,
) -> Result<SignedTransaction, TransactionError> {
  if raw.len() > MAX_BLOCK_SIZE_IN_BYTES {
    return Err(TransactionError::TooBig);
  }

  let transaction = TransactionWithSignature::from_raw(raw)?;
  context.validate_for_pool_admission(&transaction)?;
  let public = transaction.recover_public()?;

  Ok(SignedTransaction::new(public, transaction))
}
