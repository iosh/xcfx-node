//! Block preparation and automatic mining until committed results are queryable.

use std::{iter::FusedIterator, sync::Arc};

use cfx_types::{H256, U256};
use primitives::BlockNumber;
use thiserror::Error;

use crate::{
  block_producer::{RuntimeBlock, produce_block},
  chain::ChainHistory,
  fork::ForkReadError,
  production_environment::{
    PreparedProductionEnvironment, ProductionEnvironment, ProductionTimeError,
  },
  runtime_transaction::RuntimeTransaction,
  state::state_version::StateVersion,
  transaction_ingress::TransactionValidationContext,
  transaction_pool::{PoolSelectionInput, TransactionPool},
  transaction_selector::{TransactionSelection, TransactionSelectionLimits, select_transactions},
};

use super::{NodeRuntime, RuntimeCommitOutcome, load_pool_accounts_and_costs, open_state_version};

#[derive(Debug, Error)]
pub(crate) enum RuntimeBlockProductionError {
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),

  #[error(transparent)]
  Time(#[from] ProductionTimeError),

  #[error(transparent)]
  Fork(#[from] ForkReadError),

  #[error("local execution cannot advance {0} beyond the supported integer range")]
  PositionExhausted(&'static str),
}

pub(super) struct PreparedBlock {
  pub(super) block: RuntimeBlock,
  transactions_to_drop: Vec<RuntimeTransaction>,
}

struct BlockInputs {
  environment: PreparedProductionEnvironment,
  selection: TransactionSelection,
}

/// One automatic mining run, yielding each committed block before producing another.
/// Dropping the iterator stops mining without undoing earlier commits.
#[must_use = "automatic mining only produces blocks when the iterator is advanced"]
pub(crate) struct AutoMining<'a> {
  runtime: &'a mut NodeRuntime,
  finished: bool,
}

impl NodeRuntime {
  pub(crate) fn produce_and_commit_block(
    &mut self,
  ) -> Result<RuntimeCommitOutcome, RuntimeBlockProductionError> {
    let prepared = self.prepare_block(
      &self.runtime_state.history,
      self.runtime_state.effective_state.state(),
      &self.runtime_state.transaction_pool,
      &self.runtime_state.production_environment,
      Vec::new(),
      U256::zero(),
      1,
    )?;
    self.execute_and_commit_single_block_epoch_with_selection_drops(
      prepared.block,
      prepared.transactions_to_drop,
    )
  }

  /// Mines selectable transactions and pending state controls until their results
  /// are available to standard queries. Unready transactions may remain in the pool.
  /// If execution only repacks selected transactions, the run ends once earlier
  /// committed writes are visible; the repacked transactions remain in the pool.
  ///
  /// Each iteration commits at most one block. The exclusive borrow prevents
  /// intervening writes or history changes; callers can publish each outcome and
  /// stop between blocks. Construction itself does not produce a block.
  ///
  /// # Errors
  /// The first preparation or execution error ends the iterator. Earlier commits
  /// remain published; the failed block leaves the active Runtime unchanged.
  pub(crate) fn auto_mine(&mut self) -> AutoMining<'_> {
    AutoMining {
      runtime: self,
      finished: false,
    }
  }

  pub(super) fn prepare_block(
    &self,
    history: &ChainHistory,
    execution_state: &Arc<StateVersion>,
    pool: &TransactionPool,
    environment: &ProductionEnvironment,
    referee_hashes: Vec<H256>,
    nonce: U256,
    execution_block_count: usize,
  ) -> Result<PreparedBlock, RuntimeBlockProductionError> {
    let inputs = self.prepare_block_inputs(
      history,
      execution_state,
      pool,
      environment,
      execution_block_count,
    )?;
    self.build_block(history, inputs, referee_hashes, nonce)
  }

  fn prepare_block_inputs(
    &self,
    history: &ChainHistory,
    execution_state: &Arc<StateVersion>,
    pool: &TransactionPool,
    environment: &ProductionEnvironment,
    execution_block_count: usize,
  ) -> Result<BlockInputs, RuntimeBlockProductionError> {
    let parent = Arc::clone(history.optimistic_head());
    let parent_block = parent.execution_parent();

    let epoch_height =
      parent_block
        .height
        .checked_add(1)
        .ok_or(RuntimeBlockProductionError::PositionExhausted(
          "epoch height",
        ))?;
    let block_number = parent
      .pivot_block_number()
      .checked_add(execution_block_count as u64)
      .filter(|number| *number < BlockNumber::MAX)
      .ok_or(RuntimeBlockProductionError::PositionExhausted(
        "Core block number",
      ))?;

    let params = self.chain_spec.machine().params();
    let prepared_environment = environment.prepare_next_block(
      &self.production_defaults,
      parent_block.timestamp,
      parent_block.gas_limit,
      epoch_height,
      params,
    )?;

    let state = open_state_version(execution_state)?;
    let view = pool.view();
    let inputs = load_pool_accounts_and_costs(&state, &view)?;
    let selection_input = PoolSelectionInput {
      view,
      entry_states: pool.derive_entry_states(&inputs),
    };
    let spec = self.chain_spec.machine().spec(block_number, epoch_height);
    let validation = TransactionValidationContext::new(params, &spec, epoch_height);

    let selection = select_transactions(
      &selection_input,
      &parent_block,
      params,
      &validation,
      prepared_environment.block_gas_limit(),
      TransactionSelectionLimits::default(),
    );

    Ok(BlockInputs {
      environment: prepared_environment,
      selection,
    })
  }

  fn build_block(
    &self,
    history: &ChainHistory,
    inputs: BlockInputs,
    referee_hashes: Vec<H256>,
    nonce: U256,
  ) -> Result<PreparedBlock, RuntimeBlockProductionError> {
    let parent_block = history.optimistic_head().execution_parent();
    let (block_selection, transactions_to_drop) = inputs
      .selection
      .into_block_selection_and_transactions_to_drop();
    let deferred_commitment =
      history.deferred_commitment_for_header_height(block_selection.epoch_height())?;

    let runtime_block = produce_block(
      &parent_block,
      block_selection,
      self.chain_spec.machine().params(),
      inputs.environment,
      &deferred_commitment,
      referee_hashes,
      nonce,
    );

    Ok(PreparedBlock {
      block: runtime_block,
      transactions_to_drop,
    })
  }
}

impl AutoMining<'_> {
  fn mine_next_block(
    &mut self,
  ) -> Result<Option<RuntimeCommitOutcome>, RuntimeBlockProductionError> {
    let has_state_controls = !self.runtime.runtime_state.state_overlay.is_empty();
    let has_pending_results = self.has_pending_results();
    let runtime = &mut *self.runtime;
    if !has_state_controls
      && !has_pending_results
      && runtime.runtime_state.transaction_pool.len() == 0
    {
      return Ok(None);
    }

    let inputs = runtime.prepare_block_inputs(
      &runtime.runtime_state.history,
      runtime.runtime_state.effective_state.state(),
      &runtime.runtime_state.transaction_pool,
      &runtime.runtime_state.production_environment,
      1,
    )?;
    let has_pool_work = !inputs.selection.is_empty();
    let waiting_for_espace = inputs.selection.is_waiting_for_espace();
    let needs_block =
      has_state_controls || has_pending_results || has_pool_work || waiting_for_espace;
    if !needs_block {
      return Ok(None);
    }

    let previous_pool_len = runtime.runtime_state.transaction_pool.len();
    let prepared = runtime.build_block(
      &runtime.runtime_state.history,
      inputs,
      Vec::new(),
      U256::zero(),
    )?;
    let outcome = runtime.execute_and_commit_single_block_epoch_with_selection_drops(
      prepared.block,
      prepared.transactions_to_drop,
    )?;

    // With exclusive access, a commit can only remove pool entries. Unchanged
    // size means the selected work made no pool progress. Earlier results must
    // still become queryable before this run ends.
    let pool_unchanged = runtime.runtime_state.transaction_pool.len() == previous_pool_len;
    let results_visible = !self.has_pending_results();
    self.finished = has_pool_work && pool_unchanged && results_visible;

    Ok(Some(outcome))
  }

  /// Committed transaction results and state controls still awaiting `latest_state`.
  /// Empty-block protocol hooks do not extend an automatic mining run.
  fn has_pending_results(&self) -> bool {
    let history = &self.runtime.runtime_state.history;
    let latest_state = history.latest_state_height();
    history
      .views()
      .iter()
      .rev()
      .take_while(|view| view.epoch_height() > latest_state)
      .any(|view| {
        let artifacts = view
          .artifacts()
          .expect("epochs beyond latest_state must have local execution artifacts");
        artifacts.has_state_controls() || artifacts.has_executed_transactions()
      })
  }
}

impl Iterator for AutoMining<'_> {
  type Item = Result<RuntimeCommitOutcome, RuntimeBlockProductionError>;

  fn next(&mut self) -> Option<Self::Item> {
    if self.finished {
      return None;
    }

    match self.mine_next_block() {
      Ok(Some(outcome)) => Some(Ok(outcome)),
      Ok(None) => {
        self.finished = true;
        None
      }
      Err(error) => {
        self.finished = true;
        Some(Err(error))
      }
    }
  }
}

impl FusedIterator for AutoMining<'_> {}
