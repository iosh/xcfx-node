use std::sync::Arc;

use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, Space, U256};
use keccak_hash::keccak;
use primitives::{SignedTransaction, Transaction, TransactionWithSignature};
use rlp::{Encodable, RlpStream};

// Identifies the versioned Runtime encoding for impersonated transactions.
const IMPERSONATED_TRANSACTION_TAG: &[u8] = b"xcfx-node/impersonated/v1";

fn encode_impersonated_transaction(transaction: &Transaction, sender: Address) -> Vec<u8> {
  let payload = TransactionWithSignature::new_unsigned(transaction.clone());
  let payload = rlp::encode(&payload);
  let mut encoded = RlpStream::new_list(4);

  encoded.append(&IMPERSONATED_TRANSACTION_TAG);
  encoded.append(&payload);
  encoded.append(&sender);
  encoded.append(&transaction.space());

  encoded.out().to_vec()
}

pub(crate) fn fake_sign_for_execution(
  transaction: Transaction,
  sender: Address,
) -> SignedTransaction {
  let sender = sender.with_space(transaction.space());

  match transaction {
    Transaction::Native(transaction) => transaction.fake_sign_rpc(sender),
    Transaction::Ethereum(transaction) => transaction.fake_sign_rpc(sender),
  }
}

#[derive(Clone, Debug)]
pub(crate) struct RecoveredTransaction(Arc<SignedTransaction>);

impl RecoveredTransaction {
  fn new(mut transaction: SignedTransaction) -> Self {
    // Raw decoding caches wire length; block selection and queries need the
    // RLP item length used by a block body.
    transaction.transaction.rlp_size = Some(transaction.transaction.rlp_bytes().len());

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
  sender: Address,
  hash: H256,
  selection_size: usize,
}

impl ImpersonatedTransaction {
  fn new(transaction: Arc<Transaction>, sender: Address) -> Self {
    let encoded = encode_impersonated_transaction(transaction.as_ref(), sender);
    let selection_size = encoded.len();
    let hash = keccak(encoded);

    Self {
      transaction,
      sender,
      hash,
      selection_size,
    }
  }

  fn transaction(&self) -> &Transaction {
    self.transaction.as_ref()
  }

  fn sender(&self) -> Address {
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
  /// Wraps a transaction whose signature and sender have been established by the caller.
  pub(crate) fn from_recovered_signature(transaction: SignedTransaction) -> Self {
    Self::Signed(RecoveredTransaction::new(transaction))
  }

  /// Wraps a payload after the caller establishes its Runtime admission context.
  /// New submissions must check impersonation authorization; trusted replay may
  /// restore an already-admitted transaction without rechecking current state.
  pub(crate) fn from_impersonated(transaction: Transaction, sender: Address) -> Self {
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
    self.sender().with_space(self.space())
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
  /// Signed/System transactions use their block-body encoding. An impersonated
  /// transaction uses its canonical local envelope; the temporary fork shape
  /// used by validation/execution is a separate adapter detail.
  pub(crate) fn selection_size(&self) -> usize {
    match self {
      Self::Signed(transaction) => transaction.as_ref().rlp_size(),
      Self::System(transaction) => transaction.as_ref().rlp_size(),
      Self::Impersonated(transaction) => transaction.selection_size,
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
      Self::Signed(transaction) => Some(
        transaction
          .as_ref()
          .transaction
          .transaction
          .rlp_bytes()
          .to_vec(),
      ),
      Self::Impersonated(_) | Self::System(_) => None,
    }
  }

  /// Returns the wire representation used by read-only transaction queries.
  /// Impersonated signatures are placeholders: these bytes do not preserve the
  /// Runtime hash or authorize the Runtime sender when submitted as a raw transaction.
  pub(crate) fn query_raw(&self) -> Vec<u8> {
    match self {
      Self::Signed(transaction) => transaction
        .as_ref()
        .transaction
        .transaction
        .rlp_bytes()
        .to_vec(),
      Self::System(transaction) => transaction
        .as_ref()
        .transaction
        .transaction
        .rlp_bytes()
        .to_vec(),
      Self::Impersonated(transaction) => {
        primitives::transaction::TransactionWithSignatureSerializePart {
          unsigned: transaction.transaction().clone(),
          v: 0,
          r: U256::one(),
          s: U256::one(),
        }
        .rlp_bytes()
        .to_vec()
      }
    }
  }

  /// Returns the canonical encoding for an impersonated transaction.
  pub(crate) fn impersonated_encoding(&self) -> Option<Vec<u8>> {
    match self {
      Self::Impersonated(transaction) => Some(encode_impersonated_transaction(
        transaction.transaction(),
        transaction.sender(),
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
    let payload: TransactionWithSignature = rlp::decode(&payload_bytes)?;
    if !payload.is_unsigned() || !payload.is_canonical_rlp() {
      return Err(primitives::transaction::TransactionError::InvalidRlp(
        "impersonated transaction payload must be canonical and unsigned".into(),
      ));
    }

    let sender: Address = rlp.val_at(2)?;
    let space: Space = rlp.val_at(3)?;
    let transaction = payload.unsigned.clone();

    if transaction.space() != space {
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
        let signed =
          fake_sign_for_execution(transaction.transaction().clone(), transaction.sender());
        callback(&signed)
      }
    }
  }
}
