//! Coordinates execution, state controls, transaction pool updates, and recovery.

use xcfx_state::AccountNonce;
mod epoch_positions;
mod fees;
mod logs;
mod mining;
mod pool;
mod query;
mod reorg;
mod signing;
mod simulation;
mod transactions;

use crate::{
  block_producer::RuntimeBlock,
  chain::{BlockGraph, ChainHistory, EpochView},
  chain_spec::ChainSpec,
  execution::{TransactionExecutionDisposition, execute_ordered_epoch},
  fork::ForkClient,
  genesis::{GenesisError, GenesisHeaderInput, execute_genesis_with_pos},
  pos::GenesisPosDefinition,
  production_environment::{
    InvalidBlockDifficulty, ProductionDefaults, ProductionEnvironment, ProductionTimeError,
    block_gas_limit_bounds,
  },
  runtime_transaction::RuntimeTransaction,
  signing::{ImpersonationState, SigningKeyConflict, SigningKeys},
  state::{
    balance::BalanceChangeError,
    state_version::{CommittedStateVersion, StateVersion},
  },
  state_overlay::{StateControlBatch, StateControlPreparationError, StateOverlay},
  transaction_pool::{
    AccountKey, TransactionPool, TransactionPoolCheckpoint, TransactionPoolPolicy,
  },
};
use cfx_statedb::{Result as StateResult, global_params::TOTAL_GLOBAL_PARAMS};
use cfx_types::{Address, AddressWithSpace, H256, U256};
use cfxkey::KeyPair;
use epoch_positions::{ConfirmationDepths, derive_epoch_positions};
pub(crate) use epoch_positions::{EpochPosition, EpochPositionSource, EpochPositions};
pub(crate) use fees::FeeQueryError;
pub(crate) use mining::{AutoMining, BlockPreparationError, RuntimeBlockProductionError};
pub(crate) use pool::{
  AccountPoolTransactions, PoolContent, PoolQueryScope, PoolTransactionCounts,
  PoolTransactionStatus, QueuedReason,
};
pub(crate) use query::{
  EpochSelector, EspaceBlockSelector, EspaceBlockSource, EspaceReceiptSource,
  EspaceTransactionSource, LocalEspaceTransactionSource, StateQueryError, StateSelectionError,
  StateSelector,
};
pub(crate) use reorg::PoolDropReason;
pub(crate) use reorg::{ChainChange, ChainChangeReason, ReorgError};
pub(crate) use simulation::RuntimeSimulationError;
use std::{
  collections::BTreeMap,
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};
use thiserror::Error;
pub(crate) use transactions::RuntimeTransactionError;
use xcfx_state::State;

static NEXT_RUNTIME_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct RuntimeInstanceId(u64);

fn allocate_runtime_instance_id() -> RuntimeInstanceId {
  let id = NEXT_RUNTIME_INSTANCE_ID
    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
      current.checked_add(1)
    })
    .expect("Runtime instance ID space exhausted");

  RuntimeInstanceId(id)
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CheckpointId {
  runtime_instance_id: RuntimeInstanceId,
  sequence: u64,
}

pub(crate) struct TransactionPoolUpdates {
  pub(crate) transactions_to_repack: Vec<RuntimeTransaction>,
  pub(crate) transactions_dropped_during_preparation: Vec<RuntimeTransaction>,
  pub(crate) account_nonces: Vec<AccountNonce>,
}

/// Facts exposed after one Runtime transition has updated history, indexes, and the pool.
pub(crate) struct RuntimeCommitOutcome {
  pub(crate) change: ChainChange,
  pub(crate) transaction_pool_updates: TransactionPoolUpdates,
}

fn open_state_version(version: &Arc<StateVersion>) -> StateResult<State<'static>> {
  State::from_database(version.open_database())
}

/// The state currently used by Runtime consumers.
#[derive(Clone)]
struct EffectiveState {
  state: Arc<StateVersion>,
}

impl EffectiveState {
  fn from_committed(committed_state: &CommittedStateVersion) -> Self {
    Self {
      state: Arc::clone(&committed_state.version),
    }
  }

  fn prepare(
    committed_state: &CommittedStateVersion,
    overlay: &StateOverlay,
  ) -> Result<Self, StateControlPreparationError> {
    let state = overlay.prepare_effective_state(committed_state)?;

    Ok(Self { state })
  }

  fn state(&self) -> &Arc<StateVersion> {
    &self.state
  }
}

struct RuntimeState {
  history: ChainHistory,
  graph: BlockGraph,
  epoch_positions: EpochPositions,
  transaction_pool: TransactionPool,
  production_environment: ProductionEnvironment,
  state_overlay: StateOverlay,
  effective_state: EffectiveState,
}

struct ResetState {
  initial_view: Arc<EpochView>,
}

impl ResetState {
  fn build_runtime_state(
    &self,
    transaction_pool_policy: TransactionPoolPolicy,
    production_defaults: &ProductionDefaults,
    confirmation_depths: ConfirmationDepths,
  ) -> RuntimeState {
    let reset_base_timestamp = self.initial_view.execution_parent().timestamp;
    let history = ChainHistory::from_initial(Arc::clone(&self.initial_view));
    let effective_state = EffectiveState::from_committed(history.optimistic_head().state());

    RuntimeState {
      graph: BlockGraph::new(self.initial_view.execution_parent()),
      epoch_positions: derive_epoch_positions(
        &self.initial_view,
        None,
        history.latest_state_height(),
        confirmation_depths,
      ),
      history,
      transaction_pool: TransactionPool::new(transaction_pool_policy),
      production_environment: production_defaults.build_environment(reset_base_timestamp),
      state_overlay: StateOverlay::empty(),
      effective_state,
    }
  }
}

struct Checkpoint {
  history_head: Arc<EpochView>,
  graph: BlockGraph,
  epoch_positions: EpochPositions,
  transaction_pool: TransactionPoolCheckpoint,
  production_environment: ProductionEnvironment,
  state_overlay: StateOverlay,
  effective_state: EffectiveState,
}

impl Checkpoint {
  fn capture(runtime_state: &RuntimeState) -> Self {
    Self {
      history_head: Arc::clone(runtime_state.history.optimistic_head()),
      graph: runtime_state.graph.clone(),
      epoch_positions: runtime_state.epoch_positions,
      transaction_pool: runtime_state.transaction_pool.capture_checkpoint(),
      production_environment: runtime_state.production_environment,
      state_overlay: runtime_state.state_overlay.clone(),
      effective_state: runtime_state.effective_state.clone(),
    }
  }

  fn build_runtime_state(
    &self,
    current_runtime_state: &RuntimeState,
    transaction_pool_policy: TransactionPoolPolicy,
  ) -> RuntimeState {
    let history = current_runtime_state
      .history
      .rebuild_through(&self.history_head);

    RuntimeState {
      history,
      graph: self.graph.clone(),
      epoch_positions: self.epoch_positions,
      transaction_pool: TransactionPool::from_checkpoint(
        transaction_pool_policy,
        &self.transaction_pool,
      ),
      production_environment: self.production_environment,
      state_overlay: self.state_overlay.clone(),
      effective_state: self.effective_state.clone(),
    }
  }
}
/// Execution rules, signing keys, and local policies for Genesis and Fork startup.
pub(crate) struct RuntimeConfig {
  pub(crate) chain_spec: Arc<ChainSpec>,
  pub(crate) signing_keys: SigningKeys,
  pub(crate) production_defaults: ProductionDefaults,
  pub(crate) transaction_pool_policy: TransactionPoolPolicy,
  pub(crate) max_state_controls: usize,
  pub(crate) confirmed_depth: u64,
  pub(crate) finalized_depth: u64,
}

pub(crate) struct NodeRuntime {
  chain_spec: Arc<ChainSpec>,
  production_defaults: ProductionDefaults,
  signing_keys: SigningKeys,
  impersonation: ImpersonationState,
  transaction_pool_policy: TransactionPoolPolicy,
  max_state_controls: usize,
  confirmation_depths: ConfirmationDepths,
  reset_state: ResetState,
  runtime_state: RuntimeState,
  checkpoints: BTreeMap<u64, Checkpoint>,
  runtime_instance_id: RuntimeInstanceId,
  next_checkpoint_sequence: u64,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    config: RuntimeConfig,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
  ) -> Result<Self, GenesisError> {
    let genesis = execute_genesis_with_pos(
      Arc::clone(config.chain_spec.machine()),
      allocations,
      header,
      &pos_definition,
      config.chain_spec.pos_state_config(),
    )?;

    Ok(Self::from_initial(
      config,
      Arc::new(EpochView::from_genesis(genesis)),
    ))
  }

  /// Requires a base compatible with the local execution settings and validated
  /// initial globals. Funding completes before the Runtime is created. The
  /// caller retains ownership of the remote read task, including on failure.
  pub(crate) fn from_fork(
    config: RuntimeConfig,
    client: ForkClient,
    globals: [U256; TOTAL_GLOBAL_PARAMS],
    allocations: BTreeMap<AddressWithSpace, U256>,
  ) -> Result<Self, BalanceChangeError> {
    let initial_view = EpochView::from_fork(client, globals, allocations)?;
    Ok(Self::from_initial(config, Arc::new(initial_view)))
  }

  fn from_initial(config: RuntimeConfig, initial_view: Arc<EpochView>) -> Self {
    let RuntimeConfig {
      chain_spec,
      signing_keys,
      production_defaults,
      transaction_pool_policy,
      max_state_controls,
      confirmed_depth,
      finalized_depth,
    } = config;
    let confirmation_depths = ConfirmationDepths {
      confirmed: confirmed_depth,
      finalized: finalized_depth,
    };
    let reset_state = ResetState { initial_view };
    let runtime_state = reset_state.build_runtime_state(
      transaction_pool_policy,
      &production_defaults,
      confirmation_depths,
    );

    Self {
      chain_spec,
      production_defaults,
      signing_keys,
      impersonation: ImpersonationState::default(),
      transaction_pool_policy,
      max_state_controls,
      confirmation_depths,
      reset_state,
      runtime_state,
      checkpoints: BTreeMap::new(),
      runtime_instance_id: allocate_runtime_instance_id(),
      next_checkpoint_sequence: 1,
    }
  }

  /// Captures the recoverable Runtime state and returns an instance-local handle.
  /// Creating a checkpoint does not evict earlier checkpoints.
  ///
  /// # Panics
  ///
  /// Panics if the checkpoint ID sequence is exhausted.
  pub(crate) fn create_checkpoint(&mut self) -> CheckpointId {
    let next_checkpoint_sequence = self
      .next_checkpoint_sequence
      .checked_add(1)
      .expect("checkpoint ID space exhausted");

    let checkpoint_id = CheckpointId {
      runtime_instance_id: self.runtime_instance_id,
      sequence: self.next_checkpoint_sequence,
    };
    let checkpoint = Checkpoint::capture(&self.runtime_state);

    let _ = self.checkpoints.insert(checkpoint_id.sequence, checkpoint);

    self.next_checkpoint_sequence = next_checkpoint_sequence;

    checkpoint_id
  }

  pub(crate) fn revert_to_checkpoint(
    &mut self,
    checkpoint_id: CheckpointId,
  ) -> Option<ChainChange> {
    let next_runtime_state = {
      let Some(checkpoint) = self.checkpoint(checkpoint_id) else {
        return None;
      };

      checkpoint.build_runtime_state(&self.runtime_state, self.transaction_pool_policy)
    };

    let change = ChainChange::between(
      ChainChangeReason::Revert,
      &self.runtime_state,
      &next_runtime_state,
    );

    // The target and every checkpoint created after it become unavailable.
    let _invalidated_checkpoints = self.checkpoints.split_off(&checkpoint_id.sequence);

    self.runtime_state = next_runtime_state;

    Some(change)
  }

  pub(crate) fn reset(&mut self) -> ChainChange {
    let next_runtime_state = self.reset_state.build_runtime_state(
      self.transaction_pool_policy,
      &self.production_defaults,
      self.confirmation_depths,
    );

    let change = ChainChange::between(
      ChainChangeReason::Reset,
      &self.runtime_state,
      &next_runtime_state,
    );

    self.runtime_state = next_runtime_state;
    self.checkpoints.clear();
    self.impersonation.clear();

    // Keep checkpoint IDs monotonic so cleared IDs are never reused.
    change
  }

  /// Applies a validated control batch to the effective state and removes pool
  /// transactions made stale by nonce changes.
  ///
  /// Runtime state is updated only after state preparation and the pool
  /// reconciliation plan are complete.
  pub(crate) fn apply_state_control_batch(
    &mut self,
    batch: &StateControlBatch,
  ) -> Result<(), StateControlPreparationError> {
    if batch.is_empty() {
      return Ok(());
    }

    let retained = self.runtime_state.state_overlay.len();
    if retained
      .checked_add(batch.len())
      .is_none_or(|total| total > self.max_state_controls)
    {
      return Err(StateControlPreparationError::Capacity {
        retained,
        requested: batch.len(),
        limit: self.max_state_controls,
      });
    }

    let next_overlay = self.runtime_state.state_overlay.with_appended(batch);
    let next_effective_state = EffectiveState::prepare(
      self.runtime_state.history.optimistic_head().state(),
      &next_overlay,
    )?;
    let account_nonces = self.read_nonces_for_pool_reconciliation(batch, &next_effective_state)?;
    let reconciliation = self
      .runtime_state
      .transaction_pool
      .prepare_nonce_reconciliation(Vec::<H256>::new(), account_nonces);

    self.runtime_state.state_overlay = next_overlay;
    self.runtime_state.effective_state = next_effective_state;
    self
      .runtime_state
      .transaction_pool
      .apply_reconciliation(reconciliation);

    Ok(())
  }

  fn read_nonces_for_pool_reconciliation(
    &self,
    batch: &StateControlBatch,
    effective_state: &EffectiveState,
  ) -> Result<BTreeMap<AccountKey, U256>, StateControlPreparationError> {
    let nonce_targets = batch.nonce_targets();

    if nonce_targets.is_empty() {
      return Ok(BTreeMap::new());
    }

    let state = open_state_version(effective_state.state())?;
    let mut account_nonces = BTreeMap::new();

    for address in nonce_targets {
      account_nonces.insert((address.address, address.space), state.nonce(&address)?);
    }

    Ok(account_nonces)
  }

  pub(crate) fn set_author(&mut self, author: Address) -> Address {
    self.runtime_state.production_environment.set_author(author)
  }

  pub(crate) fn set_block_gas_target(&mut self, block_gas_target: u64) -> u64 {
    self
      .runtime_state
      .production_environment
      .set_block_gas_target(block_gas_target)
  }

  pub(crate) fn set_difficulty(
    &mut self,
    difficulty: U256,
  ) -> Result<U256, InvalidBlockDifficulty> {
    self
      .runtime_state
      .production_environment
      .set_difficulty(difficulty)
  }

  pub(crate) fn increase_time(&mut self, increment: u64) -> Result<u64, ProductionTimeError> {
    self
      .runtime_state
      .production_environment
      .increase_time(increment)
  }

  pub(crate) fn set_next_block_timestamp(
    &mut self,
    timestamp: u64,
  ) -> Result<u64, ProductionTimeError> {
    let parent_timestamp = self.optimistic_head_timestamp();

    self
      .runtime_state
      .production_environment
      .set_next_block_timestamp(timestamp, parent_timestamp)
  }

  fn optimistic_head_timestamp(&self) -> u64 {
    self
      .runtime_state
      .history
      .optimistic_head()
      .execution_parent()
      .timestamp
  }

  fn checkpoint(&self, checkpoint_id: CheckpointId) -> Option<&Checkpoint> {
    if checkpoint_id.runtime_instance_id != self.runtime_instance_id {
      return None;
    }

    self.checkpoints.get(&checkpoint_id.sequence)
  }

  pub(crate) fn add_signing_key(&mut self, key_pair: KeyPair) -> Result<(), SigningKeyConflict> {
    self.signing_keys.add(key_pair)
  }

  pub(crate) fn impersonate_account(&mut self, address: AddressWithSpace) -> bool {
    self.impersonation.authorize(address)
  }

  pub(crate) fn stop_impersonating_account(&mut self, address: AddressWithSpace) -> bool {
    self.impersonation.revoke(address)
  }

  pub(crate) fn is_impersonated(&self, address: AddressWithSpace) -> bool {
    self.impersonation.is_authorized(address)
  }

  /// Opens the current effective version through Conflux's `State` interface
  /// for reading.
  fn open_effective_state_for_reading(&self) -> StateResult<State<'static>> {
    open_state_version(self.runtime_state.effective_state.state())
  }

  /// Executes a constructed block and publishes its complete result on success.
  ///
  /// # Errors
  ///
  /// Returns state errors without changing the active history, pool, controls,
  /// or production environment.
  pub(crate) fn execute_and_commit_single_block_epoch(
    &mut self,
    runtime_block: RuntimeBlock,
  ) -> Result<RuntimeCommitOutcome, RuntimeBlockProductionError> {
    self.execute_and_commit_single_block_epoch_with_pool_drops(runtime_block, Vec::new())
  }

  fn execute_and_commit_single_block_epoch_with_pool_drops(
    &mut self,
    runtime_block: RuntimeBlock,
    transactions_to_drop: Vec<RuntimeTransaction>,
  ) -> Result<RuntimeCommitOutcome, RuntimeBlockProductionError> {
    let parent = Arc::clone(self.runtime_state.history.optimistic_head());
    let parent_block = parent.execution_parent();
    let block_timestamp = runtime_block.header().timestamp();
    let block_gas_limit = *runtime_block.header().gas_limit();

    let block_difficulty = *runtime_block.header().difficulty();

    assert!(
      !block_difficulty.is_zero(),
      "block difficulty must be non-zero",
    );

    assert!(
      block_timestamp >= parent_block.timestamp,
      "block timestamp {block_timestamp} must not be lower than parent timestamp {}",
      parent_block.timestamp,
    );

    let (gas_lower, gas_upper) = block_gas_limit_bounds(
      parent_block.gas_limit,
      runtime_block.header().height(),
      self.chain_spec.machine().params(),
    );

    assert!(
      block_gas_limit >= gas_lower && block_gas_limit <= gas_upper,
      "block gas limit {block_gas_limit} must be within [{gas_lower}, {gas_upper}]",
    );

    let start_block_number = parent.next_epoch_start_block_number();
    let execution_state = Arc::clone(self.runtime_state.effective_state.state());

    let (executed, account_nonces) = execute_ordered_epoch(
      self.chain_spec.machine().as_ref(),
      &parent_block,
      parent.state(),
      &execution_state,
      parent.pos_context(),
      start_block_number,
      vec![runtime_block],
    )?;

    let mut transaction_hashes_to_remove = transactions_to_drop
      .iter()
      .map(|transaction| transaction.hash())
      .collect::<Vec<_>>();
    let mut transactions_to_repack = Vec::new();

    for result in executed.transactions() {
      let transaction = result.transaction();
      match result.disposition() {
        TransactionExecutionDisposition::Executed
        | TransactionExecutionDisposition::SkippedDrop => {
          transaction_hashes_to_remove.push(transaction.hash());
        }
        TransactionExecutionDisposition::SkippedRepack => {
          transactions_to_repack.push(transaction.clone());
        }
      }
    }

    let pool_reconciliation = self
      .runtime_state
      .transaction_pool
      .prepare_nonce_reconciliation(
        transaction_hashes_to_remove,
        account_nonces
          .iter()
          .map(|update| ((update.address.address, update.address.space), update.nonce)),
      );

    let graph_block = executed.pivot_block().clone();
    let next_head = Arc::new(EpochView::from_executed_epoch(
      self.chain_spec.machine(),
      executed,
      Vec::new(),
      !self.runtime_state.state_overlay.is_empty(),
    ));
    let before_append = ChainChange::before_append(&self.runtime_state);

    self
      .runtime_state
      .graph
      .insert(graph_block)
      .expect("a locally produced block has known predecessors and a stable PoS reference");
    self
      .runtime_state
      .history
      .append_executed_epoch(Arc::clone(&next_head));
    self.runtime_state.epoch_positions = derive_epoch_positions(
      &self.reset_state.initial_view,
      Some(self.runtime_state.epoch_positions),
      self.runtime_state.history.latest_state_height(),
      self.confirmation_depths,
    );
    self
      .runtime_state
      .transaction_pool
      .apply_reconciliation(pool_reconciliation);

    self
      .runtime_state
      .production_environment
      .synchronize_committed_timestamp(block_timestamp);

    self.runtime_state.state_overlay = StateOverlay::empty();
    self.runtime_state.effective_state = EffectiveState::from_committed(next_head.state());

    Ok(RuntimeCommitOutcome {
      change: before_append.finish(&self.runtime_state),
      transaction_pool_updates: TransactionPoolUpdates {
        transactions_to_repack,
        transactions_dropped_during_preparation: transactions_to_drop,
        account_nonces,
      },
    })
  }
}
