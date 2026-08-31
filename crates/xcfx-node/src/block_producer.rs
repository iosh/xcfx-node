use crate::{
  mpt::indexed_mpt_root, production_environment::PreparedProductionEnvironment,
  runtime_transaction::RuntimeTransaction, transaction_selector::BlockTransactionSelection,
};
use cfx_executor::spec::CommonParams;
use cfx_internal_common::EpochExecutionCommitment;
use cfx_types::U256;
use primitives::{Block, BlockHeader, BlockHeaderBuilder, Cip112TransitionHeight};

/// A Runtime block keeps local transaction identity separate from the
/// optional standard fork representation.
#[derive(Clone, Debug)]
pub(crate) enum RuntimeBlock {
  /// A block whose body can be represented by the fork's standard `Block`.
  Standard {
    block: Block,
    transactions: Vec<RuntimeTransaction>,
  },
  /// A local block that intentionally has no standard fork body.
  Local {
    header: BlockHeader,
    transactions: Vec<RuntimeTransaction>,
  },
}

impl RuntimeBlock {
  /// Wraps a standard fork block and its Runtime transactions.
  pub(crate) fn from_parts(block: Block, transactions: Vec<RuntimeTransaction>) -> Self {
    assert_eq!(
      block.transactions.len(),
      transactions.len(),
      "a Runtime block and its fork Block must contain the same number of transactions",
    );

    for (runtime_transaction, block_transaction) in transactions.iter().zip(&block.transactions) {
      let runtime_fork_transaction = runtime_transaction.fork_transaction().unwrap_or_else(|| {
        panic!("a standard Runtime block cannot contain an impersonated transaction")
      });

      assert_eq!(
        runtime_transaction.hash(),
        runtime_fork_transaction.hash(),
        "a Runtime transaction must preserve its fork transaction hash",
      );

      assert_eq!(
        runtime_transaction.hash(),
        block_transaction.hash(),
        "a Runtime block and its fork Block must preserve transaction order and hashes",
      );
    }

    let transaction_hashes = transactions
      .iter()
      .map(RuntimeTransaction::hash)
      .collect::<Vec<_>>();
    let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));
    assert_eq!(
      *block.block_header.transactions_root(),
      transactions_root,
      "a standard Runtime block must commit its Runtime transaction hashes",
    );

    Self::Standard {
      block,
      transactions,
    }
  }

  /// Builds a local block without inventing a standard transaction body.
  pub(crate) fn from_local_parts(
    header: BlockHeader,
    transactions: Vec<RuntimeTransaction>,
  ) -> Self {
    assert!(
      transactions
        .iter()
        .any(|transaction| matches!(transaction, RuntimeTransaction::Impersonated(_))),
      "a local Runtime block must contain at least one impersonated transaction",
    );

    let transaction_hashes = transactions
      .iter()
      .map(RuntimeTransaction::hash)
      .collect::<Vec<_>>();
    let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

    assert_eq!(
      *header.transactions_root(),
      transactions_root,
      "a local Runtime block header must commit its Runtime transaction hashes",
    );

    Self::Local {
      header,
      transactions,
    }
  }

  /// Consumes a standard Runtime block.
  ///
  /// A local block has no fork `Block` and cannot cross this boundary.
  pub(crate) fn into_parts(self) -> (Block, Vec<RuntimeTransaction>) {
    match self {
      Self::Standard {
        block,
        transactions,
      } => (block, transactions),
      Self::Local { .. } => {
        panic!("a local Runtime block has no standard fork representation")
      }
    }
  }

  /// Returns the standard fork block, if this Runtime block has one.
  pub(crate) fn standard_block(&self) -> Option<&Block> {
    match self {
      Self::Standard { block, .. } => Some(block),
      Self::Local { .. } => None,
    }
  }

  /// Returns the block header for either block representation.
  pub(crate) fn header(&self) -> &BlockHeader {
    match self {
      Self::Standard { block, .. } => &block.block_header,
      Self::Local { header, .. } => header,
    }
  }

  /// Returns the Runtime block identity derived from its header.
  pub(crate) fn hash(&self) -> cfx_types::H256 {
    self.header().hash()
  }
  pub(crate) fn transactions(&self) -> &[RuntimeTransaction] {
    match self {
      Self::Standard { transactions, .. } | Self::Local { transactions, .. } => transactions,
    }
  }

  /// Returns a standard block for current standard-only callers.
  ///
  /// Callers that can handle local blocks should use `standard_block()` or
  /// the representation-neutral `header()`/`transactions()` methods.
  pub(crate) fn block(&self) -> &Block {
    self
      .standard_block()
      .expect("a local Runtime block cannot be used as a standard fork Block")
  }

  pub(crate) fn from_system_block(block: Block) -> Self {
    let transactions = block
      .transactions
      .iter()
      .cloned()
      .map(RuntimeTransaction::from_system)
      .collect();

    Self::from_parts(block, transactions)
  }

  /// Wraps a trusted test block whose transactions already have recovered senders.
  #[cfg(test)]
  pub(crate) fn from_recovered_block(block: Block) -> Self {
    let transactions = block
      .transactions
      .iter()
      .map(|transaction| RuntimeTransaction::from_recovered_signature(transaction.as_ref().clone()))
      .collect();

    Self::from_parts(block, transactions)
  }
}

/// Caller-controlled difficulty not yet owned by the production environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BlockProductionInput {
  pub(crate) difficulty: U256,
}

/// Constructs one linear block without executing or committing it.
pub(crate) fn produce_block(
  parent: &RuntimeBlock,
  selection: BlockTransactionSelection,
  params: &CommonParams,
  prepared_environment: PreparedProductionEnvironment,
  header_input: BlockProductionInput,
  deferred_commitment: &EpochExecutionCommitment,
) -> RuntimeBlock {
  let epoch_height = selection.epoch_height();
  let block_gas_limit = prepared_environment.block_gas_limit();
  let base_price = *selection.base_price();
  let transactions = selection.into_transactions();

  let transaction_hashes = transactions
    .iter()
    .map(RuntimeTransaction::hash)
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  let custom = params.custom_prefix(epoch_height).unwrap_or_default();
  let block_header = BlockHeaderBuilder::new()
    .with_parent_hash(parent.hash())
    .with_height(epoch_height)
    .with_timestamp(prepared_environment.timestamp())
    .with_author(prepared_environment.author())
    .with_transactions_root(transactions_root)
    .with_deferred_state_root(
      deferred_commitment
        .state_root_with_aux_info
        .aux_info
        .state_root_hash,
    )
    .with_deferred_receipts_root(deferred_commitment.receipts_root)
    .with_deferred_logs_bloom_hash(deferred_commitment.logs_bloom_hash)
    .with_blame(0)
    .with_difficulty(header_input.difficulty)
    .with_adaptive(false)
    .with_gas_limit(block_gas_limit)
    .with_referee_hashes(Vec::new())
    .with_custom(custom)
    .with_nonce(U256::zero())
    .with_pos_reference(parent.header().pos_reference().to_owned())
    .with_base_price(Some(base_price))
    .build_with_cip112(Cip112TransitionHeight::new(
      params.transition_heights.cip112,
    ));

  let standard_transactions = transactions
    .iter()
    .cloned()
    .map(RuntimeTransaction::into_fork_transaction)
    .collect::<Option<Vec<_>>>();

  match standard_transactions {
    Some(signed_transactions) => {
      RuntimeBlock::from_parts(Block::new(block_header, signed_transactions), transactions)
    }
    None => RuntimeBlock::from_local_parts(block_header, transactions),
  }
}
