//! Executes an ordered epoch against an isolated state candidate.

use std::sync::Arc;

use cfx_executor::{
  epoch_execution::{before_block_execution, before_epoch_execution},
  executive::{ExecutiveContext, TransactOptions},
  machine::Machine,
  state::State,
};
use cfx_internal_common::EpochExecutionCommitment;
use cfx_parameters::consensus::TRANSACTION_DEFAULT_EPOCH_BOUND;
use cfx_statedb::{Result as StateResult, StateDb};
use cfx_types::{H256, U256};
use cfx_vm_types::Env;
use primitives::{
  Account, Block, BlockHeaderBuilder, BlockNumber, BlockReceipts, SignedTransaction,
};
use rlp::Encodable;

use crate::{
  mpt::indexed_mpt_root,
  pos::CommittedPosState,
  state::{
    layered_mpt_state::LayeredMptState,
    state_version::{CommittedStateVersion, StateCandidate},
  },
};

pub(crate) struct ExecutedSingleBlockEpoch {
  pub(crate) block: Block,
  pub(crate) state: CommittedStateVersion,
  pub(crate) commitment: EpochExecutionCommitment,
  pub(crate) block_receipts: Vec<Arc<BlockReceipts>>,
  pub(crate) transactions_to_repack: Vec<Arc<SignedTransaction>>,
  pub(crate) accounts_for_txpool: Vec<Account>,
}

pub(crate) fn compute_epoch_receipts_root(block_receipts: &[Arc<BlockReceipts>]) -> H256 {
  let block_receipt_roots = block_receipts
    .iter()
    .map(|block_receipts| {
      let encoded_receipts = block_receipts
        .receipts
        .iter()
        .map(Encodable::rlp_bytes)
        .collect::<Vec<_>>();

      indexed_mpt_root(encoded_receipts.iter().map(|encoded| encoded.as_ref()))
    })
    .collect::<Vec<_>>();

  indexed_mpt_root(block_receipt_roots.iter().map(|root| root.as_bytes()))
}

/// Executes the current single-block epoch path.
///
/// The caller supplies an internally constructed block extending the committed
/// parent. This initial path requires the PoS reference to remain unchanged.
pub(crate) fn execute_single_block_epoch(
  machine: &Machine,
  parent_block: &Block,
  parent_state: &CommittedStateVersion,
  parent_pos_state: &CommittedPosState,
  block_number: BlockNumber,
  epoch_block: Block,
) -> ExecutedSingleBlockEpoch {
  let parent_hash = parent_block.hash();

  assert_eq!(
    parent_state.epoch_id, parent_hash,
    "ordered epoch parent state must belong to the parent block",
  );
  assert_eq!(
    epoch_block.block_header.parent_hash(),
    &parent_hash,
    "ordered epoch block must extend the supplied parent",
  );

  let expected_height = parent_block
    .block_header
    .height()
    .checked_add(1)
    .expect("a committed parent height must permit a child block");
  assert_eq!(
    epoch_block.block_header.height(),
    expected_height,
    "ordered epoch block height must follow the parent height",
  );

  let parent_pos_reference = parent_block
    .block_header
    .pos_reference()
    .as_ref()
    .expect("a committed parent block must contain a PoS reference");
  let pos_env = parent_pos_state
    .env_input(parent_pos_reference)
    .expect("committed PoS state must contain the parent block reference");

  assert_eq!(
    epoch_block.block_header.pos_reference(),
    parent_block.block_header.pos_reference(),
    "the initial ordered execution path requires a stable PoS reference",
  );

  let transaction_hashes = epoch_block
    .transactions
    .iter()
    .map(|transaction| transaction.hash())
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  assert_eq!(
    *epoch_block.block_header.transactions_root(),
    transactions_root,
    "ordered epoch transactions must match the header commitment",
  );

  let candidate = StateCandidate::new(Arc::clone(&parent_state.version));

  let (backend, state_receiver) = LayeredMptState::new(candidate);

  let database = StateDb::new(Box::new(backend));
  let mut state =
    expect_state_operation(State::new(database), "opening the committed parent state");

  expect_state_operation(
    before_epoch_execution(&mut state, machine, &epoch_block),
    "applying the epoch pre-execution transition",
  );

  let epoch_height = epoch_block.block_header.height();
  let base_gas_price = epoch_block.block_header.base_price().unwrap_or_default();
  let burnt_gas_price = base_gas_price.map_all(|price| state.burnt_gas_price(price));

  let secondary_reward = expect_state_operation(
    before_block_execution(&mut state, machine, block_number, &epoch_block),
    "applying the block pre-execution transition",
  );

  let mut env = Env {
    chain_id: machine.params().chain_id_map(epoch_height),
    number: block_number,
    author: *epoch_block.block_header.author(),
    timestamp: epoch_block.block_header.timestamp(),
    difficulty: *epoch_block.block_header.difficulty(),
    gas_limit: *epoch_block.block_header.gas_limit(),
    last_hash: parent_hash,
    accumulated_gas_used: U256::zero(),
    epoch_height,
    pos_view: Some(pos_env.pos_view),
    finalized_epoch: Some(pos_env.finalized_epoch),
    transaction_epoch_bound: TRANSACTION_DEFAULT_EPOCH_BOUND,
    base_gas_price,
    burnt_gas_price,
    transaction_hash: H256::zero(),
    ..Default::default()
  };

  let spec = machine.spec(block_number, epoch_height);
  let mut receipts = Vec::with_capacity(epoch_block.transactions.len());
  let mut execution_errors = Vec::with_capacity(epoch_block.transactions.len());
  let mut transactions_to_repack = Vec::new();

  for (transaction_index, transaction) in epoch_block.transactions.iter().enumerate() {
    env.transaction_hash = transaction.hash();

    let outcome = ExecutiveContext::new(&mut state, &env, machine, &spec)
      .transact(transaction, TransactOptions::default())
      .unwrap_or_else(|error| {
        panic!(
          "ordered epoch execution invariant violated while executing \
             transaction {transaction_index}: {error:?}"
        )
      });

    state.update_state_post_tx_execution(!spec.cip645.fix_eip1153);

    if outcome.consider_repacked() {
      transactions_to_repack.push(Arc::clone(transaction));
    }

    if let Some(burnt_fee) = outcome
      .try_as_executed()
      .and_then(|executed| executed.burnt_fee)
    {
      state.burn_by_cip1559(burnt_fee);
    }

    execution_errors.push(outcome.error_message());
    receipts.push(outcome.make_receipt(&mut env.accumulated_gas_used, &spec));
  }

  // Match the full node's historical BlockReceipts numbering behavior.
  let block_receipt = Arc::new(BlockReceipts {
    receipts,
    block_number: block_number + 1,
    secondary_reward,
    tx_execution_error_messages: execution_errors,
  });
  let block_receipts = vec![block_receipt];

  let receipts_root = compute_epoch_receipts_root(&block_receipts);
  let logs_bloom_hash = BlockHeaderBuilder::compute_block_logs_bloom_hash(&block_receipts);

  // With a stable PoS reference, the full node's post-epoch PoS distribution
  // branch is a no-op for this execution path.
  let epoch_id = epoch_block.hash();
  let commit_result = expect_state_operation(
    state.commit(epoch_id, None),
    "committing the epoch state candidate",
  );
  let committed_state = state_receiver
    .committed_state()
    .expect("a successful state commit must hand off its state version");

  assert_eq!(
    committed_state.epoch_id, epoch_id,
    "committed candidate state must use the epoch block identity",
  );
  assert_eq!(
    committed_state.version.root_with_aux_info(),
    commit_result.state_root,
    "committed candidate state must match the executor state root",
  );

  let commitment = EpochExecutionCommitment {
    state_root_with_aux_info: commit_result.state_root,
    receipts_root,
    logs_bloom_hash,
  };

  ExecutedSingleBlockEpoch {
    block: epoch_block,
    state: committed_state,
    commitment,
    block_receipts,
    transactions_to_repack,
    accounts_for_txpool: commit_result.accounts_for_txpool,
  }
}

fn expect_state_operation<T>(result: StateResult<T>, operation: &'static str) -> T {
  result.unwrap_or_else(|error| {
    panic!(
      "ordered epoch execution invariant violated while {operation}: \
         {error:?}"
    )
  })
}
