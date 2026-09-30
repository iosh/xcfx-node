//! Prepares a complete alternative execution view before publishing it.

use std::collections::{BTreeSet, HashSet};

use super::*;
use crate::chain::{GraphError, OrderedEpoch};
use crate::production_environment::ProductionTimeError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ChainChangeReason {
  Advance,
  Reorg,
  Revert,
  Reset,
}

pub(crate) enum PoolDropReason {
  Invalid(TransactionError),
  ExecutionRejected(String),
  StaleNonce,
  ReplacedBy(H256),
}

pub(crate) struct PoolDrop {
  pub(crate) hash: H256,
  pub(crate) reason: PoolDropReason,
}

/// Stable old/new views let adapters derive removed and added query results.
pub(crate) struct ChainChange {
  pub(crate) reason: ChainChangeReason,
  pub(crate) old_head: Arc<EpochView>,
  pub(crate) new_head: Arc<EpochView>,
  pub(crate) common_ancestor: Arc<EpochView>,
  pub(crate) removed: Vec<Arc<EpochView>>,
  pub(crate) added: Vec<Arc<EpochView>>,
  pub(crate) old_stability: Stability,
  pub(crate) new_stability: Stability,
  pub(crate) reinserted: Vec<H256>,
  pub(crate) dropped: Vec<PoolDrop>,
}

impl ChainChange {
  pub(super) fn between(reason: ChainChangeReason, old: &RuntimeState, new: &RuntimeState) -> Self {
    let shared = old
      .history
      .views()
      .iter()
      .zip(new.history.views())
      .take_while(|(left, right)| Arc::ptr_eq(left, right))
      .count();
    assert!(shared > 0, "view changes retain their fixed origin");
    Self {
      reason,
      old_head: Arc::clone(old.history.optimistic_head()),
      new_head: Arc::clone(new.history.optimistic_head()),
      common_ancestor: Arc::clone(&old.history.views()[shared - 1]),
      removed: old.history.views()[shared..].to_vec(),
      added: new.history.views()[shared..].to_vec(),
      old_stability: old.stability,
      new_stability: new.stability,
      reinserted: Vec::new(),
      dropped: Vec::new(),
    }
  }
}

struct PreparedHistory {
  history: ChainHistory,
  shared: usize,
  rejected: BTreeMap<H256, String>,
}

#[derive(Debug, Error)]
pub(crate) enum ReorgError {
  #[error(transparent)]
  Transaction(#[from] TransactionError),
  #[error(transparent)]
  Graph(#[from] GraphError),
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),
  #[error(transparent)]
  Control(#[from] StateControlPreparationError),
  #[error(transparent)]
  Time(#[from] ProductionTimeError),
  #[error(transparent)]
  Production(#[from] RuntimeBlockProductionError),
  #[error("the selected epoch exceeds the Core block number range")]
  BlockNumberExhausted,
  #[error("a view change cannot replace history at or below stable epoch {boundary}")]
  StableBoundary { boundary: u64 },
}

impl NodeRuntime {
  /// Builds a detached local block using the same selection and Header rules as mining.
  pub(crate) fn create_branch_block(
    &mut self,
    parent: H256,
    referees: Vec<H256>,
    transactions: Vec<RuntimeTransaction>,
    timestamp: Option<u64>,
    nonce: U256,
  ) -> Result<H256, ReorgError> {
    self.runtime_state.graph.check_capacity()?;
    let PreparedHistory { history, .. } =
      self.prepare_history(&self.runtime_state.graph, parent)?;
    let head = history.optimistic_head();
    let execution_state = if parent == self.optimistic_head().state().epoch_id {
      Arc::clone(self.runtime_state.effective_state.state())
    } else {
      Arc::clone(&head.state().version)
    };
    let mut environment = self.runtime_state.production_environment;
    if let Some(timestamp) = timestamp {
      environment.set_next_block_timestamp(timestamp, head.execution_parent().timestamp)?;
    }
    environment = environment.rebase(head.execution_parent().timestamp)?;
    let mut pool = TransactionPool::new(self.transaction_pool_policy);
    let spec = self
      .chain_spec
      .machine()
      .spec(head.pivot_block_number(), head.epoch_height());
    let validation = TransactionValidationContext::new(
      self.chain_spec.machine().params(),
      &spec,
      head.epoch_height(),
    );
    for transaction in transactions {
      validation.validate_runtime_for_pool_admission(&transaction)?;
      pool.insert(transaction)?;
    }
    let mut past = HashSet::from([self.reset_state.initial_view.state().epoch_id]);
    for view in history.views() {
      if let Some(artifacts) = view.artifacts() {
        past.extend(artifacts.all_blocks().map(RuntimeBlock::hash));
      }
    }
    let count = self
      .runtime_state
      .graph
      .execution_block_count(&referees, &past)?;
    let prepared = self.prepare_block(
      &history,
      &execution_state,
      &pool,
      &environment,
      referees,
      nonce,
      count,
    )?;
    let hash = prepared.block.hash();
    self.register_block(prepared.block)?;
    Ok(hash)
  }

  /// Registers topology without executing it or changing the active pivot.
  pub(crate) fn register_block(&mut self, block: RuntimeBlock) -> Result<bool, GraphError> {
    self.runtime_state.graph.insert(block)
  }

  /// Explicit pivot selection is a developer control, not a network fork-choice rule.
  pub(crate) fn switch_pivot(&mut self, pivot: H256) -> Result<ChainChange, ReorgError> {
    if pivot
      == self
        .runtime_state
        .history
        .optimistic_head()
        .state()
        .epoch_id
    {
      return Ok(ChainChange::between(
        ChainChangeReason::Advance,
        &self.runtime_state,
        &self.runtime_state,
      ));
    }
    self.publish_pivot(self.runtime_state.graph.clone(), pivot)
  }

  fn prepare_history(
    &self,
    graph: &BlockGraph,
    pivot: H256,
  ) -> Result<PreparedHistory, ReorgError> {
    let path = graph.pivot_path(pivot)?;
    let current = &self.runtime_state.history;
    let shared = path
      .iter()
      .zip(current.views().iter().skip(1))
      .take_while(|(hash, view)| **hash == view.state().epoch_id)
      .count();
    let ancestor = &current.views()[shared];
    let boundary = self.runtime_state.stability.reorg_boundary();
    if ancestor.epoch_height() < boundary {
      return Err(ReorgError::StableBoundary { boundary });
    }
    let mut history = current.rebuild_through(ancestor);
    let mut rejected = BTreeMap::new();
    let mut past = HashSet::from([self.reset_state.initial_view.state().epoch_id]);
    for view in history.views() {
      if let Some(artifacts) = view.artifacts() {
        past.extend(artifacts.all_blocks().map(RuntimeBlock::hash));
      }
    }

    for (offset, hash) in path.iter().skip(shared).enumerate() {
      let parent = Arc::clone(history.optimistic_head());
      let OrderedEpoch { blocks, skipped } = graph.ordered_epoch(*hash, &past)?;
      let start = parent
        .pivot_block_number()
        .checked_add(1)
        .ok_or(ReorgError::BlockNumberExhausted)?;
      start
        .checked_add(blocks.len() as u64)
        .ok_or(ReorgError::BlockNumberExhausted)?;
      let execution_state = if offset == 0 && shared + 1 == current.views().len() {
        self.runtime_state.effective_state.state()
      } else {
        &parent.state().version
      };
      let executed = execute_ordered_epoch(
        self.chain_spec.machine(),
        &parent.execution_parent(),
        parent.state(),
        execution_state,
        parent.pos_context(),
        start,
        blocks,
      )?;
      for ((block, dispositions), receipts) in executed
        .ordered_blocks
        .iter()
        .zip(&executed.transaction_dispositions)
        .zip(&executed.block_receipts)
      {
        for ((transaction, disposition), error) in block
          .transactions()
          .iter()
          .zip(dispositions)
          .zip(&receipts.tx_execution_error_messages)
        {
          if *disposition == TransactionExecutionDisposition::SkippedDrop {
            rejected.insert(transaction.hash(), error.clone());
          }
        }
      }
      past.extend(
        executed
          .ordered_blocks
          .iter()
          .chain(&skipped)
          .map(RuntimeBlock::hash),
      );
      let next = Arc::new(EpochView::from_executed_epoch(
        &parent,
        executed.ordered_blocks,
        skipped,
        executed.state,
        executed.commitment,
        executed.block_receipts,
      ));
      history.append_executed_epoch(next);
    }
    Ok(PreparedHistory {
      history,
      shared,
      rejected,
    })
  }

  fn publish_pivot(&mut self, graph: BlockGraph, pivot: H256) -> Result<ChainChange, ReorgError> {
    let PreparedHistory {
      history,
      shared,
      rejected,
    } = self.prepare_history(&graph, pivot)?;
    let current = &self.runtime_state.history;
    let old_head = Arc::clone(current.optimistic_head());
    let new_head = Arc::clone(history.optimistic_head());
    let common_ancestor = Arc::clone(&current.views()[shared]);
    let removed = current.views()[shared + 1..].to_vec();
    let added = history.views()[shared + 1..].to_vec();
    let advanced = removed.is_empty() && !added.is_empty();
    let state_overlay = if advanced {
      StateOverlay::empty()
    } else {
      self.runtime_state.state_overlay.clone()
    };
    let effective_state = EffectiveState::prepare(new_head.state(), &state_overlay)?;
    let mut production_environment = self.runtime_state.production_environment;
    if advanced {
      production_environment.synchronize_committed_timestamp(new_head.execution_parent().timestamp);
    } else {
      production_environment =
        production_environment.rebase(new_head.execution_parent().timestamp)?;
    }
    let stability = if removed.is_empty() {
      self.runtime_state.stability
    } else {
      self
        .runtime_state
        .stability
        .after_reorg(common_ancestor.epoch_height())
    };

    let (transaction_pool, reinserted, dropped) =
      self.reconcile_reorg_pool(&history, &removed, &rejected, effective_state.state())?;
    let change = ChainChange {
      reason: if removed.is_empty() {
        ChainChangeReason::Advance
      } else {
        ChainChangeReason::Reorg
      },
      old_head,
      new_head,
      common_ancestor,
      removed,
      added,
      old_stability: self.runtime_state.stability,
      new_stability: stability,
      reinserted,
      dropped,
    };

    let next = RuntimeState {
      history,
      graph,
      stability,
      transaction_pool,
      production_environment,
      state_overlay,
      effective_state,
    };
    self
      .checkpoints
      .retain(|_, checkpoint| next.history.contains_view(&checkpoint.history_head));
    self.runtime_state = next;
    Ok(change)
  }

  fn reconcile_reorg_pool(
    &self,
    history: &ChainHistory,
    removed: &[Arc<EpochView>],
    rejected: &BTreeMap<H256, String>,
    effective: &Arc<StateVersion>,
  ) -> StateResult<(TransactionPool, Vec<H256>, Vec<PoolDrop>)> {
    let head = history.optimistic_head();
    let spec = self
      .chain_spec
      .machine()
      .spec(head.pivot_block_number(), head.epoch_height());
    let validation = TransactionValidationContext::new(
      self.chain_spec.machine().params(),
      &spec,
      head.epoch_height(),
    );
    let state = open_state_version(effective)?;
    let mut pool = TransactionPool::from_checkpoint(
      self.transaction_pool_policy,
      &self.runtime_state.transaction_pool.capture_checkpoint(),
    );
    let mut dropped = Vec::new();
    for entry in pool.view().entries {
      let transaction = &entry.transaction;
      let hash = transaction.hash();
      if history.contains_mined_transaction(&hash) {
        pool.remove_by_hash(hash);
      } else if let Some(error) = rejected.get(&hash) {
        pool.remove_by_hash(hash);
        dropped.push(PoolDrop {
          hash,
          reason: PoolDropReason::ExecutionRejected(error.clone()),
        });
      } else if let Err(error) = validation.validate_runtime_for_pool_admission(transaction) {
        pool.remove_by_hash(hash);
        dropped.push(PoolDrop {
          hash,
          reason: PoolDropReason::Invalid(error),
        });
      } else if *transaction.nonce() < state.nonce(&transaction.sender_with_space())? {
        pool.remove_by_hash(hash);
        dropped.push(PoolDrop {
          hash,
          reason: PoolDropReason::StaleNonce,
        });
      }
    }

    let mut reinserted = BTreeSet::new();
    let mut reconsidered = BTreeSet::new();
    for view in removed {
      let artifacts = view
        .artifacts()
        .expect("removed local epochs have execution artifacts");
      for (block, receipts) in artifacts
        .ordered_blocks()
        .iter()
        .zip(artifacts.block_receipts())
      {
        for (transaction, receipt) in block.transactions().iter().zip(&receipts.receipts) {
          if receipt.tx_skipped() || matches!(transaction, RuntimeTransaction::System(_)) {
            continue;
          }
          let hash = transaction.hash();
          if !reconsidered.insert(hash)
            || history.contains_mined_transaction(&hash)
            || pool.get_by_hash(hash).is_some()
          {
            continue;
          }
          if let Some(error) = rejected.get(&hash) {
            dropped.push(PoolDrop {
              hash,
              reason: PoolDropReason::ExecutionRejected(error.clone()),
            });
            continue;
          }
          if let Err(error) = validation.validate_runtime_for_pool_admission(transaction) {
            dropped.push(PoolDrop {
              hash,
              reason: PoolDropReason::Invalid(error),
            });
            continue;
          }
          if *transaction.nonce() < state.nonce(&transaction.sender_with_space())? {
            dropped.push(PoolDrop {
              hash,
              reason: PoolDropReason::StaleNonce,
            });
            continue;
          }
          match pool.reinsert_after_reorg(transaction.clone()) {
            Ok(TransactionPoolInsertOutcome::Inserted) => {
              reinserted.insert(hash);
            }
            Ok(TransactionPoolInsertOutcome::Replaced { previous }) => {
              let previous_hash = previous.hash();
              reinserted.remove(&previous_hash);
              dropped.push(PoolDrop {
                hash: previous_hash,
                reason: PoolDropReason::ReplacedBy(hash),
              });
              reinserted.insert(hash);
            }
            Err(error) => dropped.push(PoolDrop {
              hash,
              reason: PoolDropReason::Invalid(error),
            }),
          }
        }
      }
    }
    // Keep balance and sponsorship read failures inside the unpublished transition.
    load_pool_accounts_and_costs(&state, &pool.view())?;
    Ok((pool, reinserted.into_iter().collect(), dropped))
  }
}
