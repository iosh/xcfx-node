//! Executes an ordered epoch against an isolated state candidate.
use std::sync::Arc;

use primitives::{Account, Block, BlockHeaderBuilder, BlockNumber, BlockReceipts};

use cfx_executor::{
  epoch_execution::{before_block_execution, before_epoch_execution},
  executive::{ExecutionOutcome, ExecutiveContext, TransactOptions},
  machine::Machine,
  state::State,
};
use cfx_parameters::consensus::TRANSACTION_DEFAULT_EPOCH_BOUND;
use cfx_statedb::{Result as StateResult, StateDb};
use cfx_types::{H256, U256};
use cfx_vm_types::{Env, Spec};
use rlp::Encodable;

use crate::{
  block_producer::{BlockParent, RuntimeBlock},
  mpt::indexed_mpt_root,
  pos::PosContext,
  runtime_transaction::RuntimeTransaction,
  state::state_version::{CommittedStateVersion, StateCandidate, StateVersion},
};

/// Known Header summaries for one execution position.
/// The state root is absent when no real commitment is available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExecutionCommitment {
  pub(crate) state_root: Option<H256>,
  pub(crate) receipts_root: H256,
  pub(crate) logs_bloom_hash: H256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransactionExecutionDisposition {
  Executed,
  SkippedDrop,
  SkippedRepack,
}

impl TransactionExecutionDisposition {
  fn from_outcome(outcome: &ExecutionOutcome) -> Self {
    match outcome {
      ExecutionOutcome::Finished(_) | ExecutionOutcome::ExecutionErrorBumpNonce(_, _) => {
        Self::Executed
      }
      ExecutionOutcome::NotExecutedDrop(_) => Self::SkippedDrop,
      ExecutionOutcome::NotExecutedToReconsiderPacking(_) => Self::SkippedRepack,
    }
  }
}

pub(crate) struct ExecutedSingleBlockEpoch {
  pub(crate) runtime_block: RuntimeBlock,
  pub(crate) state: CommittedStateVersion,
  pub(crate) commitment: ExecutionCommitment,
  pub(crate) block_receipts: Vec<Arc<BlockReceipts>>,
  pub(crate) transaction_dispositions: Vec<TransactionExecutionDisposition>,
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

fn execute_runtime_transaction(
  state: &mut State<'_>,
  env: &Env,
  machine: &Machine,
  spec: &Spec,
  transaction: &RuntimeTransaction,
) -> StateResult<ExecutionOutcome> {
  transaction.with_fork_transaction(|executor_transaction| {
    ExecutiveContext::new(state, env, machine, spec)
      .transact(executor_transaction, TransactOptions::default())
  })
}

/// Executes an internally constructed block against an isolated state.
///
/// `parent_state` supplies the committed parent identity. `execution_state`
/// may be an effective state prepared on top of that parent and therefore must
/// remain separate from the committed identity.
///
/// # Errors
///
/// Returns state errors from initialization, protocol transitions, transaction
/// execution, or state writeback. The private candidate is discarded on failure.
pub(crate) fn execute_single_block_epoch(
  machine: &Machine,
  parent_block: &BlockParent,
  parent_state: &CommittedStateVersion,
  execution_state: &Arc<StateVersion>,
  parent_pos_context: &PosContext,
  block_number: BlockNumber,
  runtime_block: RuntimeBlock,
) -> StateResult<ExecutedSingleBlockEpoch> {
  // The current fork pre-execution hooks consume only the header and block hash.
  // A local Runtime block therefore needs no invented transaction body here.
  let epoch_block = runtime_block
    .standard_block()
    .cloned()
    .unwrap_or_else(|| Block::new(runtime_block.header().clone(), Vec::new()));
  let runtime_transactions = runtime_block.transactions();

  if let Some(standard_block) = runtime_block.standard_block() {
    assert_eq!(
      standard_block.transactions.len(),
      runtime_transactions.len(),
      "a standard Runtime block must preserve every fork transaction",
    );

    for (runtime_transaction, block_transaction) in runtime_transactions
      .iter()
      .zip(&standard_block.transactions)
    {
      assert_eq!(
        runtime_transaction.hash(),
        block_transaction.hash(),
        "Runtime and fork transaction order must have matching hashes",
      );
    }
  }

  let parent_hash = parent_block.hash;

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
    .height
    .checked_add(1)
    .expect("a committed parent height must permit a child block");
  assert_eq!(
    epoch_block.block_header.height(),
    expected_height,
    "ordered epoch block height must follow the parent height",
  );

  let pos_env = parent_pos_context
    .env_input(&parent_block.pos_reference)
    .expect("committed PoS context must contain the parent block reference");

  assert_eq!(
    epoch_block.block_header.pos_reference(),
    &Some(parent_block.pos_reference),
    "the initial ordered execution path requires a stable PoS reference",
  );

  let transaction_hashes = runtime_transactions
    .iter()
    .map(RuntimeTransaction::hash)
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  assert_eq!(
    *epoch_block.block_header.transactions_root(),
    transactions_root,
    "ordered epoch transactions must match the header commitment",
  );

  let mut candidate = StateCandidate::new(Arc::clone(execution_state));
  let mut state = State::new(StateDb::new(&mut candidate))?;

  before_epoch_execution(&mut state, machine, &epoch_block)?;

  let epoch_height = epoch_block.block_header.height();
  let base_gas_price = epoch_block.block_header.base_price().unwrap_or_default();
  let burnt_gas_price = base_gas_price.map_all(|price| state.burnt_gas_price(price));

  let secondary_reward = before_block_execution(&mut state, machine, block_number, &epoch_block)?;

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
  };

  let spec = machine.spec(block_number, epoch_height);
  let mut receipts = Vec::with_capacity(runtime_transactions.len());
  let mut execution_errors = Vec::with_capacity(runtime_transactions.len());
  let mut transaction_dispositions = Vec::with_capacity(runtime_transactions.len());

  for transaction in runtime_transactions {
    env.transaction_hash = transaction.hash();

    let outcome = execute_runtime_transaction(&mut state, &env, machine, &spec, transaction)?;

    state.update_state_post_tx_execution(!spec.cip645.fix_eip1153);

    if let Some(burnt_fee) = outcome
      .try_as_executed()
      .and_then(|executed| executed.burnt_fee)
    {
      state.burn_by_cip1559(burnt_fee);
    }

    let disposition = TransactionExecutionDisposition::from_outcome(&outcome);

    transaction_dispositions.push(disposition);
    execution_errors.push(outcome.error_message());
    receipts.push(outcome.make_receipt(&mut env.accumulated_gas_used, &spec));
  }

  // Match the full node's historical BlockReceipts numbering behavior.
  let block_receipt = Arc::new(BlockReceipts {
    receipts,
    block_number: block_number
      .checked_add(1)
      .expect("the executed Core block number must permit its receipt collection number"),
    secondary_reward,
    tx_execution_error_messages: execution_errors,
  });
  let block_receipts = vec![block_receipt];

  let receipts_root = compute_epoch_receipts_root(&block_receipts);
  let logs_bloom_hash = BlockHeaderBuilder::compute_block_logs_bloom_hash(&block_receipts);

  // With a stable PoS reference, the full node's post-epoch PoS distribution
  // branch is a no-op for this execution path.
  let epoch_id = epoch_block.hash();
  let accounts_for_txpool = state.apply_changes_to_storage(None)?;
  drop(state);

  let version = Arc::new(candidate.into_version());
  let state_root = version.root_with_aux_info();
  let committed_state = CommittedStateVersion { epoch_id, version };

  let commitment = ExecutionCommitment {
    state_root: state_root.map(|root| root.aux_info.state_root_hash),
    receipts_root,
    logs_bloom_hash,
  };

  Ok(ExecutedSingleBlockEpoch {
    runtime_block,
    state: committed_state,
    commitment,
    block_receipts,
    transaction_dispositions,
    accounts_for_txpool,
  })
}
