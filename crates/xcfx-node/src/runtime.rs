//! Coordinates execution, state controls, transaction pool updates, and recovery.
mod query;
mod reorg;
mod stability;

use crate::{
  block_producer::{RuntimeBlock, produce_block},
  chain::{BlockGraph, ChainHistory, EpochHistoryView, EpochView},
  chain_spec::ChainSpec,
  execution::{ExecutedEpoch, TransactionExecutionDisposition, execute_ordered_epoch},
  fork::{ForkClient, ForkReadError},
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
  transaction_ingress::{TransactionValidationContext, decode_and_validate_raw_transaction},
  transaction_pool::{
    AccountKey, PoolAccountState, PoolEntryStates, PoolReadinessInputs, PoolSelectionInput,
    TransactionPool, TransactionPoolCheckpoint, TransactionPoolInsertOutcome,
    TransactionPoolPolicy, TransactionPoolView, pool_gas_cost, pool_transaction_cost,
  },
  transaction_selector::{TransactionSelectionLimits, select_transactions},
  virtual_execution::{
    VirtualExecutionError, VirtualExecutionOverrides, VirtualExecutionRequest,
    execute_virtual_transaction as execute_isolated_transaction,
  },
};
use cfx_executor::{executive::ExecutionOutcome, state::State};
use cfx_statedb::{Result as StateResult, global_params::TOTAL_GLOBAL_PARAMS};
use cfx_types::{
  Address, AddressSpaceUtil, AddressWithSpace, H256, Space, U256, address_util::AddressUtil,
};
use cfxkey::KeyPair;
pub(crate) use query::{EpochSelector, StateUnavailable};
pub(crate) use reorg::PoolDropReason;
pub(crate) use reorg::{ChainChange, ChainChangeReason};
pub(crate) use stability::{Stability, StabilityKind, StabilitySource, StablePosition};
use std::{
  collections::{BTreeMap, btree_map::Entry},
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};
use thiserror::Error;

use cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT;

use primitives::{
  Account, Action, BlockNumber, Transaction, block::BlockHeight, transaction::TransactionError,
};

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

#[derive(Debug, Error, Eq, PartialEq)]
pub(crate) enum RuntimeTransactionError {
  #[error(transparent)]
  Transaction(#[from] TransactionError),
  #[error("no signing key is configured for address {address:?}")]
  SigningKeyNotFound { address: AddressWithSpace },
  #[error("impersonation is not authorized for {address:?}")]
  ImpersonationNotAuthorized { address: AddressWithSpace },
  #[error("the transaction uses {transaction_space:?}, but the sender uses {sender:?}")]
  SenderSpaceMismatch {
    sender: AddressWithSpace,
    transaction_space: Space,
  },
}

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

struct PreparedBlock {
  block: RuntimeBlock,
  transactions_to_drop: Vec<RuntimeTransaction>,
}

pub(crate) struct TransactionPoolUpdates {
  pub(crate) transactions_to_repack: Vec<RuntimeTransaction>,
  pub(crate) transactions_dropped_during_selection: Vec<RuntimeTransaction>,
  pub(crate) modified_accounts: Vec<Account>,
}

/// Facts exposed after one Runtime transition has updated history, indexes, and the pool.
pub(crate) struct RuntimeCommitOutcome {
  pub(crate) change: ChainChange,
  pub(crate) optimistic_head: Arc<EpochView>,
  pub(crate) latest_state_advanced_to: Option<Arc<EpochView>>,
  pub(crate) latest_header_committed_advanced_to: Option<EpochHistoryView>,
  pub(crate) transaction_pool_updates: TransactionPoolUpdates,
}

fn open_state_version(version: &Arc<StateVersion>) -> StateResult<State<'static>> {
  State::new(version.open_database())
}

/// Derives declared gas and storage covered by sponsorship for pool readiness.
fn sponsored_gas_and_storage(
  state: &State<'_>,
  transaction: &RuntimeTransaction,
) -> StateResult<(U256, u64)> {
  let Transaction::Native(native_transaction) = transaction.transaction() else {
    return Ok(Default::default());
  };

  let contract_address = match native_transaction.action() {
    Action::Call(address) if address.is_contract_address() => address,
    _ => return Ok(Default::default()),
  };

  let Some(sponsor_info) = state.sponsor_info(&contract_address)? else {
    return Ok(Default::default());
  };

  let sender = transaction.sender();

  if !state.check_contract_whitelist(&contract_address, &sender)? {
    return Ok(Default::default());
  }

  let declared_gas_cost = pool_gas_cost(*transaction.gas(), *transaction.gas_price());
  let sponsored_gas = if declared_gas_cost <= sponsor_info.sponsor_gas_bound
    && declared_gas_cost <= sponsor_info.sponsor_balance_for_gas
  {
    *native_transaction.gas()
  } else {
    U256::zero()
  };

  let required_storage_collateral =
    U256::from(*native_transaction.storage_limit()) * *DRIPS_PER_STORAGE_COLLATERAL_UNIT;
  let sponsored_storage = if required_storage_collateral
    <= sponsor_info.sponsor_balance_for_collateral + sponsor_info.unused_storage_points()
  {
    *native_transaction.storage_limit()
  } else {
    0
  };

  Ok((sponsored_gas, sponsored_storage))
}

fn load_pool_accounts_and_costs(
  state: &State<'_>,
  pool_view: &TransactionPoolView,
) -> StateResult<PoolReadinessInputs> {
  let mut account_states = BTreeMap::<AccountKey, PoolAccountState>::new();
  let mut transaction_costs = BTreeMap::new();

  for entry in &pool_view.entries {
    let transaction = &entry.transaction;
    let sender = transaction.sender();
    let space = transaction.space();
    let account_key = (sender, space);

    if let Entry::Vacant(entry) = account_states.entry(account_key) {
      let address = sender.with_space(space);

      entry.insert(PoolAccountState {
        state_nonce: state.nonce(&address)?,
        balance: state.balance(&address)?,
      });
    }

    let (sponsored_gas, sponsored_storage) = sponsored_gas_and_storage(state, transaction)?;
    let transaction_cost = pool_transaction_cost(transaction, sponsored_gas, sponsored_storage);

    transaction_costs.insert(transaction.hash(), transaction_cost);
  }

  Ok(PoolReadinessInputs::new(account_states, transaction_costs))
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
  stability: Stability,
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
  ) -> RuntimeState {
    let reset_base_timestamp = self.initial_view.execution_parent().timestamp;
    let history = ChainHistory::from_initial(Arc::clone(&self.initial_view));
    let effective_state = EffectiveState::from_committed(history.optimistic_head().state());

    RuntimeState {
      graph: BlockGraph::new(self.initial_view.execution_parent()),
      stability: Stability::initial(&self.initial_view),
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
  stability: Stability,
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
      stability: runtime_state.stability,
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
      stability: self.stability,
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
}

pub(crate) struct NodeRuntime {
  chain_spec: Arc<ChainSpec>,
  production_defaults: ProductionDefaults,
  signing_keys: SigningKeys,
  impersonation: ImpersonationState,
  transaction_pool_policy: TransactionPoolPolicy,
  max_state_controls: usize,
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
    } = config;
    let reset_state = ResetState { initial_view };
    let runtime_state =
      reset_state.build_runtime_state(transaction_pool_policy, &production_defaults);

    Self {
      chain_spec,
      production_defaults,
      signing_keys,
      impersonation: ImpersonationState::default(),
      transaction_pool_policy,
      max_state_controls,
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

    assert!(
      !self.checkpoints.contains_key(&checkpoint_id.sequence),
      "a checkpoint sequence must never be reused",
    );

    let _previous = self.checkpoints.insert(checkpoint_id.sequence, checkpoint);

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
    let next_runtime_state = self
      .reset_state
      .build_runtime_state(self.transaction_pool_policy, &self.production_defaults);

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

  /// Executes one fully resolved transaction against the current effective
  /// state without publishing any state transition.
  pub(crate) fn execute_virtual_transaction(
    &self,
    request: VirtualExecutionRequest,
    overrides: VirtualExecutionOverrides,
  ) -> Result<ExecutionOutcome, VirtualExecutionError> {
    let parent = self.runtime_state.history.optimistic_head();

    execute_isolated_transaction(
      self.chain_spec.machine().as_ref(),
      &parent.execution_parent(),
      parent.pos_context(),
      self.runtime_state.effective_state.state(),
      parent.next_epoch_start_block_number(),
      request,
      overrides,
    )
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

  pub(crate) fn can_sign_for(&self, address: AddressWithSpace) -> bool {
    self.signing_keys.can_sign_for(address)
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

  fn with_current_transaction_validation<T>(
    &self,
    callback: impl FnOnce(&TransactionValidationContext<'_>) -> T,
  ) -> T {
    let optimistic_head = self.runtime_state.history.optimistic_head();
    let epoch_height = self.runtime_state.history.optimistic_height();
    let block_number = optimistic_head.pivot_block_number();
    let params = self.chain_spec.machine().params();
    let spec = self.chain_spec.machine().spec(block_number, epoch_height);
    let validation = TransactionValidationContext::new(params, &spec, epoch_height);

    callback(&validation)
  }

  fn validate_runtime_transaction_for_pool(
    &self,
    transaction: &RuntimeTransaction,
  ) -> Result<(), TransactionError> {
    self.with_current_transaction_validation(|validation| {
      validation.validate_runtime_for_pool_admission(transaction)
    })
  }

  fn validate_sender_space(
    sender: AddressWithSpace,
    transaction: &Transaction,
  ) -> Result<(), RuntimeTransactionError> {
    if transaction.space() != sender.space {
      return Err(RuntimeTransactionError::SenderSpaceMismatch {
        sender,
        transaction_space: transaction.space(),
      });
    }

    Ok(())
  }

  /// Signs a payload with an instance-owned key and returns a Runtime
  /// transaction backed by that signature. This method does not insert into
  /// the pool.
  pub(crate) fn sign_transaction(
    &self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Result<RuntimeTransaction, RuntimeTransactionError> {
    Self::validate_sender_space(sender, &transaction)?;
    let transaction = self
      .signing_keys
      .sign(sender, transaction)
      .ok_or(RuntimeTransactionError::SigningKeyNotFound { address: sender })?;

    Ok(RuntimeTransaction::from_recovered_signature(transaction))
  }

  /// Signs, validates, and admits a transaction using an instance-owned key.
  pub(crate) fn submit_node_transaction(
    &mut self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Result<H256, RuntimeTransactionError> {
    let transaction = self.sign_transaction(sender, transaction)?;
    self.validate_runtime_transaction_for_pool(&transaction)?;
    let transaction_hash = transaction.hash();
    self.insert_admitted_transaction(transaction)?;
    Ok(transaction_hash)
  }

  /// Builds an impersonated transaction only when the sender is currently
  /// authorized. The returned value belongs to this Runtime instance.
  pub(crate) fn build_impersonated_transaction(
    &self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Result<RuntimeTransaction, RuntimeTransactionError> {
    Self::validate_sender_space(sender, &transaction)?;
    if !self.impersonation.is_authorized(sender) {
      return Err(RuntimeTransactionError::ImpersonationNotAuthorized { address: sender });
    }

    Ok(RuntimeTransaction::from_impersonated(transaction, sender))
  }

  /// Checks authorization, validates, and admits an impersonated transaction.
  pub(crate) fn submit_impersonated_transaction(
    &mut self,
    sender: AddressWithSpace,
    transaction: Transaction,
  ) -> Result<H256, RuntimeTransactionError> {
    let transaction = self.build_impersonated_transaction(sender, transaction)?;
    self.validate_runtime_transaction_for_pool(&transaction)?;
    let transaction_hash = transaction.hash();
    self.insert_admitted_transaction(transaction)?;
    Ok(transaction_hash)
  }

  pub(crate) fn transaction_pool_selection_input(&self) -> StateResult<PoolSelectionInput> {
    let view = self.transaction_pool_view();
    let inputs = self.pool_readiness_inputs(&view)?;
    let entry_states = self.derive_transaction_pool_states(&inputs);

    Ok(PoolSelectionInput { view, entry_states })
  }

  /// Validates raw transaction bytes against the current optimistic view and
  /// inserts the signed Runtime transaction into this Runtime's pool.
  pub(crate) fn submit_raw_transaction(&mut self, raw: &[u8]) -> Result<H256, TransactionError> {
    let transaction = {
      let optimistic_head = self.runtime_state.history.optimistic_head();
      let epoch_height = self.runtime_state.history.optimistic_height();
      let block_number = optimistic_head.pivot_block_number();
      let params = self.chain_spec.machine().params();
      let spec = self.chain_spec.machine().spec(block_number, epoch_height);
      let validation = TransactionValidationContext::new(params, &spec, epoch_height);

      decode_and_validate_raw_transaction(raw, &validation)?
    };

    let transaction_hash = transaction.hash();
    self.insert_admitted_transaction(transaction)?;

    Ok(transaction_hash)
  }

  /// Inserts a transaction that has already passed transaction-only ingress validation.
  pub(crate) fn insert_admitted_transaction(
    &mut self,
    transaction: RuntimeTransaction,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    let transaction_hash = transaction.hash();

    if self
      .runtime_state
      .history
      .contains_mined_transaction(&transaction_hash)
    {
      return Err(TransactionError::AlreadyImported);
    }

    self.runtime_state.transaction_pool.insert(transaction)
  }

  /// Returns a stable snapshot of the transaction pool.
  pub(crate) fn transaction_pool_view(&self) -> TransactionPoolView {
    self.runtime_state.transaction_pool.view()
  }

  pub(crate) fn derive_transaction_pool_states(
    &self,
    inputs: &PoolReadinessInputs,
  ) -> PoolEntryStates {
    self
      .runtime_state
      .transaction_pool
      .derive_entry_states(inputs)
  }

  pub(crate) fn remove_stale_transactions(
    &mut self,
    entry_states: &PoolEntryStates,
  ) -> Vec<RuntimeTransaction> {
    self
      .runtime_state
      .transaction_pool
      .remove_stale(entry_states)
  }

  pub(crate) fn pool_readiness_inputs(
    &self,
    pool_view: &TransactionPoolView,
  ) -> StateResult<PoolReadinessInputs> {
    let state = self.open_effective_state_for_reading()?;
    load_pool_accounts_and_costs(&state, pool_view)
  }

  /// Opens the current effective version through Conflux's `State` interface
  /// for reading.
  fn open_effective_state_for_reading(&self) -> StateResult<State<'static>> {
    open_state_version(self.runtime_state.effective_state.state())
  }

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

  fn prepare_block(
    &self,
    history: &ChainHistory,
    execution_state: &Arc<StateVersion>,
    pool: &TransactionPool,
    environment: &ProductionEnvironment,
    referee_hashes: Vec<H256>,
    nonce: U256,
    execution_block_count: usize,
  ) -> Result<PreparedBlock, RuntimeBlockProductionError> {
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

    let (block_selection, transactions_to_drop) =
      selection.into_block_selection_and_transactions_to_drop();

    let deferred_commitment = history.deferred_commitment_for_header_height(epoch_height)?;

    let runtime_block = produce_block(
      &parent_block,
      block_selection,
      params,
      prepared_environment,
      &deferred_commitment,
      referee_hashes,
      nonce,
    );

    Ok(PreparedBlock {
      block: runtime_block,
      transactions_to_drop,
    })
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
    self.execute_and_commit_single_block_epoch_with_selection_drops(runtime_block, Vec::new())
  }

  fn execute_and_commit_single_block_epoch_with_selection_drops(
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

    let previous_latest_state_height = self.runtime_state.history.latest_state_height();
    let previous_latest_header_committed_height =
      self.runtime_state.history.latest_header_committed_height();
    let start_block_number = parent.next_epoch_start_block_number();
    let execution_state = Arc::clone(self.runtime_state.effective_state.state());

    let executed = execute_ordered_epoch(
      self.chain_spec.machine().as_ref(),
      &parent_block,
      parent.state(),
      &execution_state,
      parent.pos_context(),
      start_block_number,
      vec![runtime_block],
    )?;

    let ExecutedEpoch {
      mut ordered_blocks,
      state,
      commitment,
      block_receipts,
      transaction_dispositions,
      accounts_for_txpool,
    } = executed;

    let runtime_block = ordered_blocks
      .pop()
      .expect("single-block execution returns its block");
    let transaction_dispositions = transaction_dispositions
      .into_iter()
      .next()
      .expect("single-block execution returns its transaction dispositions");
    let mut transaction_hashes_to_remove = transactions_to_drop
      .iter()
      .map(|transaction| transaction.hash())
      .collect::<Vec<_>>();
    let mut transactions_to_repack = Vec::new();

    assert_eq!(
      runtime_block.transactions().len(),
      transaction_dispositions.len(),
      "every Runtime block transaction must have one execution disposition",
    );

    for (transaction, disposition) in runtime_block
      .transactions()
      .iter()
      .zip(transaction_dispositions)
    {
      match disposition {
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
      .prepare_reconciliation(transaction_hashes_to_remove, &accounts_for_txpool);

    self
      .runtime_state
      .graph
      .insert(runtime_block.clone())
      .expect("a locally produced block has known predecessors and a stable PoS reference");
    let next_head = Arc::new(EpochView::from_executed_epoch(
      &parent,
      vec![runtime_block],
      Vec::new(),
      state,
      commitment,
      block_receipts,
    ));

    self
      .runtime_state
      .history
      .append_executed_epoch(Arc::clone(&next_head));
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

    let latest_state_advanced_to = (self.runtime_state.history.latest_state_height()
      > previous_latest_state_height)
      .then(|| Arc::clone(self.runtime_state.history.latest_state_epoch()));
    let latest_header_committed_advanced_to =
      (self.runtime_state.history.latest_header_committed_height()
        > previous_latest_header_committed_height)
        .then(|| self.runtime_state.history.latest_header_committed_epoch());

    Ok(RuntimeCommitOutcome {
      change: ChainChange {
        reason: ChainChangeReason::Advance,
        old_head: Arc::clone(&parent),
        new_head: Arc::clone(&next_head),
        common_ancestor: parent,
        removed: Vec::new(),
        added: vec![Arc::clone(&next_head)],
        old_stability: self.runtime_state.stability,
        new_stability: self.runtime_state.stability,
        reinserted: Vec::new(),
        dropped: Vec::new(),
      },
      optimistic_head: next_head,
      latest_state_advanced_to,
      latest_header_committed_advanced_to,
      transaction_pool_updates: TransactionPoolUpdates {
        transactions_to_repack,
        transactions_dropped_during_selection: transactions_to_drop,
        modified_accounts: accounts_for_txpool,
      },
    })
  }
}
