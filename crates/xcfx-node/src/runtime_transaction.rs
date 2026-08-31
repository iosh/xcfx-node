use std::sync::Arc;

use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, Space, U256};
use cfxkey::public_to_address;
use keccak_hash::keccak;
use primitives::{SignedTransaction, Transaction, TransactionWithSignature};
use rlp::RlpStream;

// Identifies the versioned Runtime encoding for impersonated transactions.
const IMPERSONATED_TRANSACTION_TAG: &[u8] = b"xcfx-node/impersonated/v1";

fn encode_impersonated_transaction(transaction: &Transaction, sender: AddressWithSpace) -> Vec<u8> {
  let payload = TransactionWithSignature::new_unsigned(transaction.clone());
  let payload = rlp::encode(&payload);
  let mut encoded = RlpStream::new_list(4);

  encoded.append(&IMPERSONATED_TRANSACTION_TAG);
  encoded.append(&payload);
  encoded.append(&sender.address);
  encoded.append(&sender.space);

  encoded.out().to_vec()
}

fn compute_impersonated_hash(transaction: &Transaction, sender: AddressWithSpace) -> H256 {
  keccak(encode_impersonated_transaction(transaction, sender))
}

#[derive(Clone, Debug)]
pub(crate) struct RecoveredTransaction(Arc<SignedTransaction>);

impl RecoveredTransaction {
  fn new(transaction: SignedTransaction) -> Self {
    assert!(
      !transaction.is_unsigned(),
      "a recovered transaction must be signed"
    );

    let public = transaction
      .public
      .as_ref()
      .expect("a recovered transaction must contain a public key");
    assert!(
      transaction
        .verify_public(false)
        .expect("a recovered transaction signature must be verifiable"),
      "a recovered transaction must contain a valid signature",
    );

    let expected_sender = public_to_address(public, transaction.space() == Space::Native);
    assert_eq!(
      transaction.sender, expected_sender,
      "a recovered transaction sender must be derived from its public key",
    );

    Self(Arc::new(transaction))
  }

  fn as_ref(&self) -> &SignedTransaction {
    self.0.as_ref()
  }

  fn into_arc(self) -> Arc<SignedTransaction> {
    self.0
  }
}

#[derive(Clone, Debug)]
pub(crate) struct ImpersonatedTransaction {
  transaction: Arc<Transaction>,
  sender: AddressWithSpace,
  hash: H256,
}

impl ImpersonatedTransaction {
  fn new(transaction: Arc<Transaction>, sender: AddressWithSpace) -> Self {
    let hash = compute_impersonated_hash(transaction.as_ref(), sender);

    Self {
      transaction,
      sender,
      hash,
    }
  }

  fn transaction(&self) -> &Transaction {
    self.transaction.as_ref()
  }

  fn sender(&self) -> Address {
    self.sender.address
  }

  fn sender_with_space(&self) -> AddressWithSpace {
    self.sender
  }

  fn hash(&self) -> H256 {
    self.hash
  }
}

#[derive(Clone, Debug)]
pub(crate) struct SystemTransaction(Arc<SignedTransaction>);

impl SystemTransaction {
  fn as_ref(&self) -> &SignedTransaction {
    self.0.as_ref()
  }

  fn into_arc(self) -> Arc<SignedTransaction> {
    self.0
  }
}

#[derive(Clone, Debug)]
pub(crate) enum RuntimeTransaction {
  Signed(RecoveredTransaction),
  Impersonated(ImpersonatedTransaction),
  System(SystemTransaction),
}

impl RuntimeTransaction {
  /// Wraps a transaction after ingress validation and sender recovery.
  pub(crate) fn from_recovered_signature(transaction: SignedTransaction) -> Self {
    Self::Signed(RecoveredTransaction::new(transaction))
  }

  /// Wraps a payload after the caller establishes its Runtime admission context.
  /// New submissions must check impersonation authorization; trusted replay may
  /// restore an already-admitted transaction without rechecking current state.
  pub(crate) fn from_impersonated(transaction: Transaction, sender: AddressWithSpace) -> Self {
    assert_eq!(
      transaction.space(),
      sender.space,
      "an impersonated sender must use the transaction space",
    );
    Self::Impersonated(ImpersonatedTransaction::new(Arc::new(transaction), sender))
  }

  /// Wraps a transaction constructed by a controlled system path.
  pub(crate) fn from_system(transaction: Arc<SignedTransaction>) -> Self {
    Self::System(SystemTransaction(transaction))
  }

  pub(crate) fn sender(&self) -> Address {
    match self {
      Self::Signed(transaction) => transaction.as_ref().sender,
      Self::Impersonated(transaction) => transaction.sender(),
      Self::System(transaction) => transaction.as_ref().sender,
    }
  }

  /// Returns the stable identity hash used by this local Runtime.
  ///
  /// For an impersonated transaction this is not a standard Conflux wire hash.
  pub(crate) fn hash(&self) -> H256 {
    match self {
      Self::Signed(transaction) => transaction.as_ref().hash(),
      Self::Impersonated(transaction) => transaction.hash(),
      Self::System(transaction) => transaction.as_ref().hash(),
    }
  }

  pub(crate) fn space(&self) -> Space {
    self.transaction().space()
  }

  pub(crate) fn sender_with_space(&self) -> AddressWithSpace {
    match self {
      Self::Signed(transaction) => transaction
        .as_ref()
        .sender
        .with_space(transaction.as_ref().space()),
      Self::Impersonated(transaction) => transaction.sender_with_space(),
      Self::System(transaction) => transaction
        .as_ref()
        .sender
        .with_space(transaction.as_ref().space()),
    }
  }

  pub(crate) fn nonce(&self) -> &U256 {
    self.transaction().nonce()
  }

  pub(crate) fn gas(&self) -> &U256 {
    self.transaction().gas()
  }

  pub(crate) fn gas_price(&self) -> &U256 {
    self.transaction().gas_price()
  }

  pub(crate) fn value(&self) -> &U256 {
    self.transaction().value()
  }

  pub(crate) fn storage_limit(&self) -> Option<u64> {
    self.transaction().storage_limit()
  }

  /// Returns the size charged by local block selection.
  ///
  /// Signed/System transactions use their standard encoding. An impersonated
  /// transaction uses its canonical local envelope; the temporary fork shape
  /// used by validation/execution is a separate adapter detail.
  pub(crate) fn selection_size(&self) -> usize {
    match self {
      Self::Signed(transaction) => transaction.as_ref().rlp_size(),
      Self::System(transaction) => transaction.as_ref().rlp_size(),
      Self::Impersonated(transaction) => {
        encode_impersonated_transaction(transaction.transaction(), transaction.sender_with_space())
          .len()
      }
    }
  }

  pub(crate) fn transaction(&self) -> &Transaction {
    match self {
      Self::Signed(transaction) => &transaction.as_ref().unsigned,
      Self::Impersonated(transaction) => transaction.transaction(),
      Self::System(transaction) => &transaction.as_ref().unsigned,
    }
  }

  /// Returns the fork representation when this transaction has one.
  ///
  /// Impersonated transactions intentionally have no standard fork
  /// representation.
  pub(crate) fn fork_transaction(&self) -> Option<&SignedTransaction> {
    match self {
      Self::Signed(transaction) => Some(transaction.as_ref()),
      Self::Impersonated(_) => None,
      Self::System(transaction) => Some(transaction.as_ref()),
    }
  }

  /// Consumes a Runtime transaction with its fork representation, if present.
  pub(crate) fn into_fork_transaction(self) -> Option<Arc<SignedTransaction>> {
    match self {
      Self::Signed(transaction) => Some(transaction.into_arc()),
      Self::Impersonated(_) => None,
      Self::System(transaction) => Some(transaction.into_arc()),
    }
  }

  /// Returns canonical bytes only for a genuine signed user transaction.
  pub(crate) fn standard_raw(&self) -> Option<Vec<u8>> {
    match self {
      Self::Signed(transaction) => Some(rlp::encode(&transaction.as_ref().transaction).to_vec()),
      Self::Impersonated(_) | Self::System(_) => None,
    }
  }

  /// Returns the canonical encoding for an impersonated transaction.
  pub(crate) fn impersonated_encoding(&self) -> Option<Vec<u8>> {
    match self {
      Self::Impersonated(transaction) => Some(encode_impersonated_transaction(
        transaction.transaction(),
        transaction.sender_with_space(),
      )),
      Self::Signed(_) | Self::System(_) => None,
    }
  }

  /// Decodes the canonical representation used for an impersonated transaction.
  /// This is a replay/query boundary, not a raw ingress path and does not
  /// re-authorize the sender.
  pub(crate) fn decode_impersonated_transaction(
    raw: &[u8],
  ) -> Result<Self, primitives::transaction::TransactionError> {
    let rlp = rlp::Rlp::new(raw);
    if !rlp.is_list() || rlp.item_count()? != 4 {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "impersonated transaction encoding must be a four-item list".into(),
      ));
    }

    let tag: Vec<u8> = rlp.val_at(0)?;
    if tag.as_slice() != IMPERSONATED_TRANSACTION_TAG {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "unknown impersonated transaction tag".into(),
      ));
    }

    let payload_bytes: Vec<u8> = rlp.val_at(1)?;
    let payload = TransactionWithSignature::from_raw(&payload_bytes)?;
    if !payload.is_unsigned() || !payload.is_canonical_rlp() {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "impersonated transaction payload must be canonical and unsigned".into(),
      ));
    }

    let address = rlp.val_at(2)?;
    let space = rlp.val_at(3)?;
    let sender = AddressWithSpace { address, space };
    let transaction = payload.unsigned.clone();

    if transaction.space() != sender.space {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "impersonated transaction sender space does not match payload".into(),
      ));
    }

    let expected = encode_impersonated_transaction(&transaction, sender);
    if expected != raw {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "impersonated transaction encoding is not canonical".into(),
      ));
    }

    Ok(Self::from_impersonated(transaction, sender))
  }

  /// Gives a controlled caller a fork-compatible signed view.
  ///
  /// For an impersonated transaction the view is constructed on the stack and
  /// cannot escape the callback. It must not be used as a Runtime hash, block
  /// body, query raw bytes, or proof of a real signature.
  pub(crate) fn with_fork_transaction<T>(
    &self,
    callback: impl FnOnce(&SignedTransaction) -> T,
  ) -> T {
    match self {
      Self::Signed(transaction) => callback(transaction.as_ref()),
      Self::System(transaction) => callback(transaction.as_ref()),
      Self::Impersonated(transaction) => {
        let sender = transaction.sender_with_space();
        let signed = match transaction.transaction().clone() {
          Transaction::Native(transaction) => transaction.fake_sign_rpc(sender),
          Transaction::Ethereum(transaction) => transaction.fake_sign_rpc(sender),
        };
        callback(&signed)
      }
    }
  }
}
