use cfx_executor::spec::CommonParams;
use cfx_internal_common::EpochExecutionCommitment;
use cfx_types::{Address, U256};
use primitives::{Block, BlockHeaderBuilder, Cip112TransitionHeight};

use crate::{
  mpt::indexed_mpt_root, transaction_pool::TransactionPoolView,
  transaction_selector::TransactionSelectionPlan,
};

/// Caller-controlled fields for one deterministic Local block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalBlockHeaderInput {
  pub(crate) timestamp: u64,
  pub(crate) author: Address,
  pub(crate) difficulty: U256,
}

/// A produced block kept together with the selection identity that created it.
pub(crate) struct ProducedBlockCandidate {
  selection: TransactionSelectionPlan,
  block: Block,
}

impl ProducedBlockCandidate {
  pub(crate) fn matches_pool_view(&self, view: &TransactionPoolView) -> bool {
    self.selection.matches_pool_view(view)
  }

  pub(crate) fn into_parts(self) -> (TransactionSelectionPlan, Block) {
    (self.selection, self.block)
  }
}

/// Constructs one linear Local block without executing or committing it.
pub(crate) fn produce_local_block(
  parent: &Block,
  selection: TransactionSelectionPlan,
  params: &CommonParams,
  header_input: LocalBlockHeaderInput,
  deferred_commitment: &EpochExecutionCommitment,
) -> ProducedBlockCandidate {
  let transactions = selection.transactions().to_vec();

  let transaction_hashes = transactions
    .iter()
    .map(|transaction| transaction.hash())
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  let height = selection.epoch_height();
  let timestamp = header_input.timestamp.max(parent.block_header.timestamp());

  let custom = params.custom_prefix(height).unwrap_or_default();

  let block_header = BlockHeaderBuilder::new()
    .with_parent_hash(parent.hash())
    .with_height(height)
    .with_timestamp(timestamp)
    .with_author(header_input.author)
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
    .with_gas_limit(selection.block_gas_limit())
    .with_referee_hashes(Vec::new())
    .with_custom(custom)
    .with_nonce(U256::zero())
    .with_pos_reference(parent.block_header.pos_reference().to_owned())
    .with_base_price(Some(*selection.base_price()))
    .build_with_cip112(Cip112TransitionHeight::new(
      params.transition_heights.cip112,
    ));

  let block = Block::new(block_header, transactions);

  ProducedBlockCandidate { selection, block }
}
