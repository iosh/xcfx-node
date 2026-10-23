//! Executes an ordered epoch against an isolated state candidate.
use std::{borrow::Cow, sync::Arc};

use primitives::{Account, Block, BlockHeaderBuilder, BlockNumber, BlockReceipts};

use cfx_executor::{
  epoch_execution::{before_block_execution, before_epoch_execution},
  executive::{ExecutionOutcome, ExecutiveContext, TransactOptions},
  internal_contract::{block_hash_slot, epoch_hash_slot},
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

/// Coupled blocks and execution context from one successful epoch execution.
/// Only the executor constructs it.
pub(crate) struct ExecutedEpoch {
  start_block_number: BlockNumber,
  blocks: Vec<ExecutedBlock>,
  state: CommittedStateVersion,
  commitment: ExecutionCommitment,
  pos_context: PosContext,
}

/// Block-local correspondence is established by the executor's transaction loop.
pub(crate) struct ExecutedBlock {
  block: RuntimeBlock,
  receipts: Arc<BlockReceipts>,
  dispositions: Vec<TransactionExecutionDisposition>,
}

/// Borrows the transaction and its disposition from the same execution.
pub(crate) struct ExecutedTransactionRef<'a> {
  block: &'a ExecutedBlock,
  index: usize,
}

impl ExecutedEpoch {
  pub(crate) fn pivot_block(&self) -> &RuntimeBlock {
    &self
      .blocks
      .last()
      .expect("a successful epoch execution retains its pivot block")
      .block
  }

  pub(crate) fn transactions(&self) -> impl Iterator<Item = ExecutedTransactionRef<'_>> {
    self.blocks.iter().flat_map(|block| {
      (0..block.block.transactions().len())
        .map(move |index| ExecutedTransactionRef { block, index })
    })
  }

  /// Transfers the execution context together with its paired block results.
  pub(crate) fn into_parts(
    self,
  ) -> (
    BlockNumber,
    Vec<ExecutedBlock>,
    CommittedStateVersion,
    ExecutionCommitment,
    PosContext,
  ) {
    (
      self.start_block_number,
      self.blocks,
      self.state,
      self.commitment,
      self.pos_context,
    )
  }
}

impl ExecutedBlock {
  /// Retains the paired block and receipts after transient dispositions have been consumed.
  pub(crate) fn into_parts(self) -> (RuntimeBlock, Arc<BlockReceipts>) {
    (self.block, self.receipts)
  }
}

impl<'a> ExecutedTransactionRef<'a> {
  pub(crate) fn transaction(&self) -> &'a RuntimeTransaction {
    &self.block.block.transactions()[self.index]
  }

  pub(crate) fn disposition(&self) -> TransactionExecutionDisposition {
    self.block.dispositions[self.index]
  }

  pub(crate) fn execution_error_message(&self) -> Option<&'a str> {
    let error = &self.block.receipts.tx_execution_error_messages[self.index];
    (!error.is_empty()).then_some(error.as_str())
  }
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

/// System-storage entries needed before executing on a local Genesis state.
/// Genesis cannot include its own hash in the state used to compute that hash.
/// Fork history belongs to the remote state and must not be initialized here.
pub(crate) fn genesis_hash_storage_entries(
  machine: &Machine,
  parent_block: &BlockParent,
  parent_state: &StateVersion,
) -> impl Iterator<Item = ([u8; 32], U256)> {
  let local_genesis = parent_block.height == 0 && matches!(parent_state, StateVersion::Mpt(_));
  let params = machine.params();
  let genesis_hash = U256::from_big_endian(parent_block.hash.as_bytes());

  [
    (local_genesis && params.transition_numbers.cip133b == 0)
      .then_some((block_hash_slot(0), genesis_hash)),
    (local_genesis && params.transition_heights.cip133e == 0)
      .then_some((epoch_hash_slot(0), genesis_hash)),
  ]
  .into_iter()
  .flatten()
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

/// Executes the graph's ordered blocks in one private state candidate.
///
/// The pivot is last. Its parent supplies the execution identity, while
/// `execution_state` may include local state controls on that parent.
/// Account updates are returned separately for pool reconciliation and commit reporting.
///
/// # Errors
///
/// Any database or protocol-transition error discards the whole epoch.
pub(crate) fn execute_ordered_epoch(
  machine: &Machine,
  parent_block: &BlockParent,
  parent_state: &CommittedStateVersion,
  execution_state: &Arc<StateVersion>,
  parent_pos_context: &PosContext,
  start_block_number: BlockNumber,
  ordered_blocks: Vec<RuntimeBlock>,
) -> StateResult<(ExecutedEpoch, Vec<Account>)> {
  let pivot = ordered_blocks
    .last()
    .expect("an ordered epoch contains its pivot");
  assert_eq!(
    parent_state.epoch_id, parent_block.hash,
    "the parent state must belong to the execution parent"
  );
  assert_eq!(
    *pivot.header().parent_hash(),
    parent_block.hash,
    "the pivot must extend the execution parent"
  );
  assert_eq!(
    pivot.header().height(),
    parent_block.height + 1,
    "the pivot must advance the epoch height"
  );

  let pos_env = parent_pos_context
    .env_input(&parent_block.pos_reference)
    .expect("the parent PoS context must resolve its reference");
  let epoch_height = pivot.header().height();
  let epoch_timestamp = pivot.header().timestamp();
  let epoch_id = pivot.hash();
  let mut candidate = StateCandidate::new(Arc::clone(execution_state));
  let mut state = State::new(StateDb::new(&mut candidate))?;

  for (slot, hash) in genesis_hash_storage_entries(machine, parent_block, &parent_state.version) {
    state.set_system_storage(slot.into(), hash)?;
  }

  before_epoch_execution(&mut state, machine, protocol_block(pivot).as_ref())?;

  let base_gas_price = pivot.header().base_price().unwrap_or_default();
  let burnt_gas_price = base_gas_price.map_all(|price| state.burnt_gas_price(price));
  let mut executed_blocks = Vec::with_capacity(ordered_blocks.len());
  let mut last_hash = parent_block.hash;

  for (index, runtime_block) in ordered_blocks.into_iter().enumerate() {
    assert_eq!(
      *runtime_block.header().pos_reference(),
      Some(parent_block.pos_reference),
      "ordered execution requires a stable PoS reference",
    );
    let block_number = start_block_number
      .checked_add(index as u64)
      .expect("a prepared epoch must fit the Core block number range");
    let secondary_reward = before_block_execution(
      &mut state,
      machine,
      block_number,
      protocol_block(&runtime_block).as_ref(),
    )?;
    let header = runtime_block.header();
    let mut env = Env {
      chain_id: machine.params().chain_id_map(epoch_height),
      number: block_number,
      author: *header.author(),
      timestamp: epoch_timestamp,
      difficulty: *header.difficulty(),
      gas_limit: *header.gas_limit(),
      last_hash,
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
    let mut receipts = Vec::with_capacity(runtime_block.transactions().len());
    let mut execution_errors = Vec::with_capacity(runtime_block.transactions().len());
    let mut dispositions = Vec::with_capacity(runtime_block.transactions().len());

    for transaction in runtime_block.transactions() {
      env.transaction_hash = transaction.hash();
      let outcome = execute_runtime_transaction(&mut state, &env, machine, &spec, transaction)?;
      state.update_state_post_tx_execution(!spec.cip645.fix_eip1153);
      if let Some(burnt_fee) = outcome
        .try_as_executed()
        .and_then(|executed| executed.burnt_fee)
      {
        state.burn_by_cip1559(burnt_fee);
      }
      dispositions.push(TransactionExecutionDisposition::from_outcome(&outcome));
      execution_errors.push(outcome.error_message());
      receipts.push(outcome.make_receipt(&mut env.accumulated_gas_used, &spec));
    }

    let receipts = Arc::new(BlockReceipts {
      receipts,
      // The full node numbers each receipt collection one after its VM block.
      block_number: block_number
        .checked_add(1)
        .expect("the receipt number must fit"),
      secondary_reward,
      tx_execution_error_messages: execution_errors,
    });
    last_hash = runtime_block.hash();
    executed_blocks.push(ExecutedBlock {
      block: runtime_block,
      receipts,
      dispositions,
    });
  }

  let modified_accounts = state.apply_changes_to_storage(None)?;
  drop(state);
  let version = Arc::new(candidate.into_version());
  let block_receipts = executed_blocks
    .iter()
    .map(|block| Arc::clone(&block.receipts))
    .collect::<Vec<_>>();
  let commitment = ExecutionCommitment {
    state_root: version
      .root_with_aux_info()
      .map(|root| root.aux_info.state_root_hash),
    receipts_root: compute_epoch_receipts_root(&block_receipts),
    logs_bloom_hash: BlockHeaderBuilder::compute_block_logs_bloom_hash(&block_receipts),
  };
  Ok((
    ExecutedEpoch {
      start_block_number,
      blocks: executed_blocks,
      state: CommittedStateVersion { epoch_id, version },
      commitment,
      pos_context: parent_pos_context.clone(),
    },
    modified_accounts,
  ))
}

/// Shared protocol hooks consume the header/hash, not local transaction metadata.
fn protocol_block(block: &RuntimeBlock) -> Cow<'_, Block> {
  match block.standard_block() {
    Some(block) => Cow::Borrowed(block),
    None => Cow::Owned(Block::new(block.header().clone(), Vec::new())),
  }
}
