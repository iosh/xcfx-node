//! Owns one node instance's authoritative protocol state.
use crate::{
  block_producer::{BlockProductionInput, RuntimeBlock, produce_block},
  execution::{
    ExecutedSingleBlockEpoch, TransactionExecutionDisposition, execute_single_block_epoch,
  },
  genesis::{
    ExecutedGenesis, ExecutedGenesisWithPos, GenesisError, GenesisHeaderInput,
    execute_genesis_with_pos,
  },
  pos::{CommittedPosState, GenesisPosDefinition},
  production_environment::{
    PreparedProductionEnvironment, ProductionDefaults, ProductionEnvironment, ProductionTimeError,
  },
  runtime_transaction::RuntimeTransaction,
  signing::{ImpersonationState, SigningKeyConflict, SigningKeys},
  state::{
    layered_mpt_state::LayeredMptState,
    state_version::{CommittedStateVersion, StateCandidate, StateVersion},
  },
  transaction_ingress::{TransactionValidationContext, decode_and_validate_raw_transaction},
  transaction_pool::{
    AccountKey, PoolAccountState, PoolEntryStates, PoolReadinessInputs, PoolSelectionInput,
    TransactionPool, TransactionPoolCheckpoint, TransactionPoolInsertOutcome,
    TransactionPoolPolicy, TransactionPoolView, pool_gas_cost, pool_transaction_cost,
  },
  transaction_selector::{TransactionSelectionLimits, select_transactions},
};
use cfx_executor::{machine::Machine, spec::CommonParams, state::State};
use cfx_statedb::{Result as StateResult, StateDb};
use cfx_types::{
  AddressSpaceUtil, AddressWithSpace, AllChainID, H256, Space, U256, address_util::AddressUtil,
};
use cfx_vm_types::Spec;
use cfxkey::KeyPair;
use diem_types::term_state::pos_state_config::PosStateConfig;
use std::{
  collections::{BTreeMap, HashMap, btree_map::Entry},
  sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
  },
};
use thiserror::Error;

use cfx_internal_common::EpochExecutionCommitment;
use cfx_parameters::{
  consensus::{DEFERRED_STATE_EPOCH_COUNT, TRANSACTION_DEFAULT_EPOCH_BOUND},
  staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT,
};

use primitives::{
  Account, Action, Block, BlockNumber, BlockReceipts, Receipt, Transaction, block::BlockHeight,
  transaction::TransactionError,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CheckpointPolicy {
  max_checkpoints: usize,
}

impl CheckpointPolicy {
  pub(crate) const fn new(max_checkpoints: usize) -> Self {
    Self { max_checkpoints }
  }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("checkpoint limit reached ({max_checkpoints})")]
pub(crate) struct CheckpointLimitError {
  pub(crate) max_checkpoints: usize,
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
}

pub(crate) struct CommittedEpoch {
  start_block_number: BlockNumber,
  ordered_blocks: Vec<RuntimeBlock>,
  commitment: EpochExecutionCommitment,
  block_receipts: Vec<Arc<BlockReceipts>>,
}
impl CommittedEpoch {
  pub(crate) fn pivot_runtime_block(&self) -> &RuntimeBlock {
    self
      .ordered_blocks
      .last()
      .expect("a committed epoch always contains a pivot block")
  }

  /// Returns the pivot only for callers that explicitly require a fork Block.
  pub(crate) fn pivot_block(&self) -> &Block {
    self.pivot_runtime_block().block()
  }

  pub(crate) fn commitment(&self) -> &EpochExecutionCommitment {
    &self.commitment
  }

  pub(crate) fn block_receipts(&self) -> &[Arc<BlockReceipts>] {
    &self.block_receipts
  }

  fn next_epoch_start_block_number(&self) -> BlockNumber {
    let epoch_size = BlockNumber::try_from(self.ordered_blocks.len())
      .expect("a committed epoch size must fit in BlockNumber");

    self
      .start_block_number
      .checked_add(epoch_size)
      .expect("a committed epoch must permit a subsequent block number")
  }

  fn pivot_block_number(&self) -> BlockNumber {
    self
      .next_epoch_start_block_number()
      .checked_sub(1)
      .expect("a committed epoch always contains at least one block")
  }
}
pub(crate) struct CommittedChainView {
  epoch: CommittedEpoch,
  state: CommittedStateVersion,
  pos_state: Arc<CommittedPosState>,
}

impl CommittedChainView {
  fn from_genesis(genesis: ExecutedGenesisWithPos) -> Self {
    let ExecutedGenesisWithPos {
      execution,
      committed_pos_state,
    } = genesis;

    let ExecutedGenesis {
      block,
      committed_state,
      commitment,
      block_receipts,
    } = execution;

    Self {
      epoch: CommittedEpoch {
        start_block_number: 0,
        ordered_blocks: vec![RuntimeBlock::from_system_block(block)],
        commitment,
        block_receipts,
      },
      state: committed_state,
      pos_state: Arc::new(committed_pos_state),
    }
  }
  pub(crate) fn epoch(&self) -> &CommittedEpoch {
    &self.epoch
  }

  pub(crate) fn state(&self) -> &CommittedStateVersion {
    &self.state
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockLocation {
  epoch_height: BlockHeight,
  block_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TransactionLocation {
  block: BlockLocation,
  transaction_index: usize,
}

#[derive(Clone)]
pub(crate) struct MinedBlockView {
  chain_view: Arc<CommittedChainView>,
  block_index: usize,
}
impl MinedBlockView {
  pub(crate) fn runtime_block(&self) -> &RuntimeBlock {
    self
      .chain_view
      .epoch
      .ordered_blocks
      .get(self.block_index)
      .expect("an indexed block must exist in its committed chain view")
  }

  /// Returns the standard fork block for standard-only callers.
  pub(crate) fn block(&self) -> &Block {
    self.runtime_block().block()
  }

  pub(crate) fn standard_block(&self) -> Option<&Block> {
    self.runtime_block().standard_block()
  }

  pub(crate) fn hash(&self) -> H256 {
    self.runtime_block().hash()
  }

  pub(crate) fn transactions(&self) -> &[RuntimeTransaction] {
    self.runtime_block().transactions()
  }

  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self
      .chain_view
      .epoch
      .pivot_runtime_block()
      .header()
      .height()
  }
}

#[derive(Clone)]
pub(crate) struct MinedTransactionView {
  block: MinedBlockView,
  transaction_index: usize,
}

impl MinedTransactionView {
  pub(crate) fn block(&self) -> &MinedBlockView {
    &self.block
  }

  pub(crate) fn transaction(&self) -> &RuntimeTransaction {
    self
      .block
      .transactions()
      .get(self.transaction_index)
      .expect("an indexed transaction must exist in its mined block")
  }

  pub(crate) fn transaction_index(&self) -> usize {
    self.transaction_index
  }

  pub(crate) fn hash(&self) -> H256 {
    self.transaction().hash()
  }

  pub(crate) fn sender(&self) -> AddressWithSpace {
    self.transaction().sender_with_space()
  }

  pub(crate) fn standard_raw(&self) -> Option<Vec<u8>> {
    self.transaction().standard_raw()
  }

  pub(crate) fn impersonated_encoding(&self) -> Option<Vec<u8>> {
    self.transaction().impersonated_encoding()
  }
}

#[derive(Clone)]
pub(crate) struct TransactionReceiptView {
  transaction: MinedTransactionView,
}

impl TransactionReceiptView {
  fn new(transaction: MinedTransactionView) -> Option<Self> {
    let block_receipts = Self::block_receipts_for(&transaction);
    let has_receipt = block_receipts
      .receipts
      .get(transaction.transaction_index)
      .is_some();
    let has_error_entry = block_receipts
      .tx_execution_error_messages
      .get(transaction.transaction_index)
      .is_some();

    assert_eq!(
      has_receipt, has_error_entry,
      "a transaction execution receipt and error entry must be present together",
    );

    has_receipt.then_some(Self { transaction })
  }

  pub(crate) fn transaction(&self) -> &MinedTransactionView {
    &self.transaction
  }

  pub(crate) fn receipt(&self) -> &Receipt {
    self
      .block_receipts()
      .receipts
      .get(self.transaction.transaction_index)
      .expect("an indexed transaction receipt must exist in its committed block results")
  }

  pub(crate) fn execution_error_message(&self) -> Option<&str> {
    let message = self
      .block_receipts()
      .tx_execution_error_messages
      .get(self.transaction.transaction_index)
      .expect("an indexed transaction error must exist in its committed block results");

    (!message.is_empty()).then_some(message.as_str())
  }

  fn block_receipts(&self) -> &BlockReceipts {
    Self::block_receipts_for(&self.transaction)
  }

  fn block_receipts_for(transaction: &MinedTransactionView) -> &BlockReceipts {
    transaction
      .block
      .chain_view
      .epoch
      .block_receipts
      .get(transaction.block.block_index)
      .expect("an indexed block must have committed receipts")
  }
}

struct CommittedChainHistory {
  views: Vec<Arc<CommittedChainView>>,
  block_locations: HashMap<H256, BlockLocation>,
  transaction_locations: HashMap<H256, TransactionLocation>,
}

impl CommittedChainHistory {
  fn from_genesis(genesis: Arc<CommittedChainView>) -> Self {
    assert_eq!(
      genesis.epoch.start_block_number, 0,
      "the Genesis epoch must start at block number zero",
    );
    assert_eq!(
      genesis.epoch.ordered_blocks.len(),
      1,
      "the Genesis epoch must contain exactly one block",
    );
    assert_eq!(
      genesis.epoch.block_receipts.len(),
      1,
      "the Genesis epoch must contain exactly one block receipt collection",
    );

    let runtime_block = &genesis.epoch.ordered_blocks[0];
    let block = runtime_block.block();
    let runtime_transactions = runtime_block.transactions();
    let block_receipts = &genesis.epoch.block_receipts[0];

    assert_eq!(
      block.transactions.len(),
      runtime_transactions.len(),
      "the Genesis Runtime block must preserve every fork transaction",
    );

    assert_eq!(
      block.block_header.height(),
      0,
      "the Genesis block height must be zero",
    );
    assert_eq!(
      block_receipts.block_number, 0,
      "the Genesis receipt collection must belong to block number zero",
    );
    assert!(
      block_receipts.receipts.is_empty() && block_receipts.tx_execution_error_messages.is_empty(),
      "Genesis initialization transactions must not have execution receipt entries",
    );

    let block_location = BlockLocation {
      epoch_height: 0,
      block_index: 0,
    };
    let block_locations = HashMap::from([(block.hash(), block_location)]);
    let mut transaction_locations = HashMap::with_capacity(block.transactions.len());

    for (transaction_index, (transaction, runtime_transaction)) in block
      .transactions
      .iter()
      .zip(runtime_transactions)
      .enumerate()
    {
      assert_eq!(
        transaction.hash(),
        runtime_transaction.hash(),
        "the Genesis Runtime block must preserve transaction hashes",
      );
      let transaction_location = TransactionLocation {
        block: block_location,
        transaction_index,
      };

      assert!(
        transaction_locations
          .insert(transaction.hash(), transaction_location)
          .is_none(),
        "the Genesis block must not contain duplicate transaction hashes",
      );
    }

    Self {
      views: vec![genesis],
      block_locations,
      transaction_locations,
    }
  }
  fn capture_checkpoint_head(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.optimistic_head())
  }

  fn rebuild_through_checkpoint_head(&self, checkpoint_head: &Arc<CommittedChainView>) -> Self {
    let checkpoint_height = checkpoint_head
      .epoch
      .pivot_runtime_block()
      .header()
      .height();
    let checkpoint_index =
      usize::try_from(checkpoint_height).expect("a checkpoint epoch height must fit in usize");
    let ancestor = self
      .views
      .get(checkpoint_index)
      .expect("a checkpoint history head must exist in the current history");

    assert!(
      Arc::ptr_eq(ancestor, checkpoint_head),
      "a checkpoint history head must be an ancestor of the current history",
    );

    let checkpoint_views = &self.views[..=checkpoint_index];
    let (genesis, remaining_views) = checkpoint_views
      .split_first()
      .expect("a checkpoint history must contain the Genesis view");

    let mut history = Self::from_genesis(Arc::clone(genesis));

    for view in remaining_views {
      history.append_executed_epoch(Arc::clone(view));
    }

    history
  }

  fn optimistic_head(&self) -> &Arc<CommittedChainView> {
    self
      .views
      .last()
      .expect("committed chain history always contains the Genesis view")
  }

  fn optimistic_height(&self) -> BlockHeight {
    self
      .optimistic_head()
      .epoch
      .pivot_runtime_block()
      .header()
      .height()
  }

  fn append_executed_epoch(&mut self, view: Arc<CommittedChainView>) {
    let epoch_height = view.epoch.pivot_runtime_block().header().height();
    let expected_epoch_height = BlockHeight::try_from(self.views.len())
      .expect("a linear history length must fit in BlockHeight");

    assert_eq!(
      epoch_height, expected_epoch_height,
      "an executed epoch append must contain the next epoch height",
    );
    assert_eq!(
      view.epoch.ordered_blocks.len(),
      view.epoch.block_receipts.len(),
      "every executed block must have one block receipt collection",
    );

    let mut block_locations = HashMap::with_capacity(view.epoch.ordered_blocks.len());
    let mut transaction_locations = HashMap::new();

    for (block_index, (runtime_block, block_receipts)) in view
      .epoch
      .ordered_blocks
      .iter()
      .zip(&view.epoch.block_receipts)
      .enumerate()
    {
      let runtime_transactions = runtime_block.transactions();

      assert_eq!(
        runtime_transactions.len(),
        block_receipts.receipts.len(),
        "every executed block transaction must have one receipt",
      );
      assert_eq!(
        runtime_transactions.len(),
        block_receipts.tx_execution_error_messages.len(),
        "every executed block transaction must have one execution error entry",
      );

      if let Some(block) = runtime_block.standard_block() {
        assert_eq!(
          block.transactions.len(),
          runtime_transactions.len(),
          "a standard Runtime block must preserve every fork transaction",
        );

        for (fork_transaction, runtime_transaction) in
          block.transactions.iter().zip(runtime_transactions)
        {
          assert_eq!(
            fork_transaction.hash(),
            runtime_transaction.hash(),
            "a committed Runtime block must preserve transaction hashes",
          );
        }
      }

      let block_hash = runtime_block.hash();
      let block_location = BlockLocation {
        epoch_height,
        block_index,
      };

      assert!(
        !self.block_locations.contains_key(&block_hash),
        "a committed block hash must not already exist in history",
      );
      assert!(
        block_locations.insert(block_hash, block_location).is_none(),
        "a committed epoch must not contain duplicate block hashes",
      );

      for (transaction_index, receipt) in block_receipts.receipts.iter().enumerate() {
        if receipt.tx_skipped() {
          continue;
        }

        let transaction_hash = runtime_transactions[transaction_index].hash();
        let transaction_location = TransactionLocation {
          block: block_location,
          transaction_index,
        };

        assert!(
          !self.transaction_locations.contains_key(&transaction_hash),
          "an executed transaction hash must not already exist in history",
        );
        assert!(
          transaction_locations
            .insert(transaction_hash, transaction_location)
            .is_none(),
          "a committed epoch must not execute a transaction hash twice",
        );
      }
    }

    self.views.reserve(1);
    self.block_locations.reserve(block_locations.len());
    self
      .transaction_locations
      .reserve(transaction_locations.len());

    self.views.push(view);
    self.block_locations.extend(block_locations);
    self.transaction_locations.extend(transaction_locations);
  }

  fn view_at_epoch_height(&self, epoch_height: BlockHeight) -> Option<&Arc<CommittedChainView>> {
    let index = usize::try_from(epoch_height).ok()?;
    let view = self.views.get(index)?;

    if view.epoch.pivot_runtime_block().header().height() == epoch_height {
      Some(view)
    } else {
      None
    }
  }

  fn latest_state_height(&self) -> BlockHeight {
    let optimistic_height = self.optimistic_height();

    if optimistic_height < DEFERRED_STATE_EPOCH_COUNT {
      0
    } else {
      optimistic_height - DEFERRED_STATE_EPOCH_COUNT + 1
    }
  }

  fn latest_state_view(&self) -> &Arc<CommittedChainView> {
    self
      .view_at_epoch_height(self.latest_state_height())
      .expect("a complete linear history must contain its latest state view")
  }

  fn latest_header_committed_height(&self) -> BlockHeight {
    self
      .optimistic_height()
      .saturating_sub(DEFERRED_STATE_EPOCH_COUNT)
  }

  fn latest_header_committed_view(&self) -> &Arc<CommittedChainView> {
    self
      .view_at_epoch_height(self.latest_header_committed_height())
      .expect("a complete linear history must contain its latest Header-committed view")
  }

  fn contains_mined_transaction(&self, transaction_hash: &H256) -> bool {
    self.transaction_locations.contains_key(transaction_hash)
  }

  fn mined_block_at(&self, location: BlockLocation) -> MinedBlockView {
    let chain_view = Arc::clone(
      self
        .view_at_epoch_height(location.epoch_height)
        .expect("an indexed block must reference a committed epoch"),
    );

    chain_view
      .epoch
      .ordered_blocks
      .get(location.block_index)
      .expect("an indexed block must reference a block in its committed epoch");

    MinedBlockView {
      chain_view,
      block_index: location.block_index,
    }
  }

  fn mined_block_by_hash(&self, block_hash: &H256) -> Option<MinedBlockView> {
    self
      .block_locations
      .get(block_hash)
      .copied()
      .map(|location| self.mined_block_at(location))
  }

  fn mined_transaction_at(&self, location: TransactionLocation) -> MinedTransactionView {
    let block = self.mined_block_at(location.block);

    block
      .transactions()
      .get(location.transaction_index)
      .expect("an indexed transaction must reference its mined block body");

    MinedTransactionView {
      block,
      transaction_index: location.transaction_index,
    }
  }

  fn mined_transaction_by_hash(&self, transaction_hash: &H256) -> Option<MinedTransactionView> {
    self
      .transaction_locations
      .get(transaction_hash)
      .copied()
      .map(|location| self.mined_transaction_at(location))
  }

  fn transaction_receipt_by_hash(&self, transaction_hash: &H256) -> Option<TransactionReceiptView> {
    let location = *self.transaction_locations.get(transaction_hash)?;

    if location.block.epoch_height > self.latest_state_height() {
      return None;
    }

    TransactionReceiptView::new(self.mined_transaction_at(location))
  }

  fn deferred_commitment_for_header_height(
    &self,
    header_height: BlockHeight,
  ) -> &EpochExecutionCommitment {
    let deferred_epoch_height = header_height.saturating_sub(DEFERRED_STATE_EPOCH_COUNT);

    self
      .view_at_epoch_height(deferred_epoch_height)
      .expect("a next linear epoch must have its deferred commitment in committed history")
      .epoch
      .commitment()
  }
}

pub(crate) struct TransactionPoolUpdates {
  pub(crate) transactions_to_repack: Vec<RuntimeTransaction>,
  pub(crate) transactions_dropped_during_selection: Vec<RuntimeTransaction>,
  pub(crate) modified_accounts: Vec<Account>,
}

/// Facts exposed after one Runtime transition has updated history, indexes, and the pool.
pub(crate) struct RuntimeCommitOutcome {
  pub(crate) optimistic_view: Arc<CommittedChainView>,
  pub(crate) latest_state_advanced_to: Option<Arc<CommittedChainView>>,
  pub(crate) latest_header_committed_advanced_to: Option<Arc<CommittedChainView>>,
  pub(crate) transaction_pool_updates: TransactionPoolUpdates,
}

fn open_committed_state(version: &Arc<StateVersion>) -> StateResult<State> {
  let (backend, _) = LayeredMptState::new(StateCandidate::new(Arc::clone(version)));
  State::new(StateDb::new(Box::new(backend)))
}

/// Derives declared gas and storage covered by sponsorship for pool readiness.
fn sponsored_gas_and_storage(
  state: &State,
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

fn transaction_validation_context<'a>(
  params: &'a CommonParams,
  spec: &'a Spec,
  height: BlockHeight,
) -> TransactionValidationContext<'a> {
  TransactionValidationContext {
    chain_id: AllChainID::new(
      params.chain_id(height, Space::Native),
      params.chain_id(height, Space::Ethereum),
    ),
    height,
    transitions: &params.transition_heights,
    transaction_epoch_bound: TRANSACTION_DEFAULT_EPOCH_BOUND,
    max_nonce: None,
    spec,
  }
}

struct RuntimeState {
  history: CommittedChainHistory,
  transaction_pool: TransactionPool,
  production_environment: ProductionEnvironment,
}

struct ResetState {
  genesis_view: Arc<CommittedChainView>,
}

impl ResetState {
  fn from_genesis(genesis_view: Arc<CommittedChainView>) -> Self {
    Self { genesis_view }
  }

  fn build_runtime_state(
    &self,
    transaction_pool_policy: TransactionPoolPolicy,
    production_defaults: &ProductionDefaults,
  ) -> RuntimeState {
    let reset_base_timestamp = self
      .genesis_view
      .epoch
      .pivot_runtime_block()
      .header()
      .timestamp();

    RuntimeState {
      history: CommittedChainHistory::from_genesis(Arc::clone(&self.genesis_view)),
      transaction_pool: TransactionPool::new(transaction_pool_policy),
      production_environment: production_defaults.build_environment(reset_base_timestamp),
    }
  }
}

struct Checkpoint {
  history_head: Arc<CommittedChainView>,
  transaction_pool: TransactionPoolCheckpoint,
  production_environment: ProductionEnvironment,
}

impl Checkpoint {
  fn capture(runtime_state: &RuntimeState) -> Self {
    Self {
      history_head: runtime_state.history.capture_checkpoint_head(),
      transaction_pool: runtime_state.transaction_pool.capture_checkpoint(),
      production_environment: runtime_state.production_environment,
    }
  }

  fn build_runtime_state(
    &self,
    current_runtime_state: &RuntimeState,
    transaction_pool_policy: TransactionPoolPolicy,
  ) -> RuntimeState {
    RuntimeState {
      history: current_runtime_state
        .history
        .rebuild_through_checkpoint_head(&self.history_head),
      transaction_pool: TransactionPool::from_checkpoint(
        transaction_pool_policy,
        &self.transaction_pool,
      ),
      production_environment: self.production_environment,
    }
  }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RevertCheckpointOutcome {
  Reverted,
  Unavailable,
}
pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  pos_config: PosStateConfig,
  production_defaults: ProductionDefaults,
  signing_keys: SigningKeys,
  impersonation: ImpersonationState,
  transaction_pool_policy: TransactionPoolPolicy,
  checkpoint_policy: CheckpointPolicy,
  reset_state: ResetState,
  runtime_state: RuntimeState,
  checkpoints: BTreeMap<u64, Checkpoint>,
  runtime_instance_id: RuntimeInstanceId,
  next_checkpoint_sequence: u64,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    machine: Arc<Machine>,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
    pos_config: PosStateConfig,
    production_defaults: ProductionDefaults,
    transaction_pool_policy: TransactionPoolPolicy,
    checkpoint_policy: CheckpointPolicy,
  ) -> Result<Self, GenesisError> {
    let genesis = execute_genesis_with_pos(
      Arc::clone(&machine),
      allocations,
      header,
      &pos_definition,
      &pos_config,
    )?;

    let reset_state = ResetState::from_genesis(Arc::new(CommittedChainView::from_genesis(genesis)));
    let runtime_state =
      reset_state.build_runtime_state(transaction_pool_policy, &production_defaults);

    Ok(Self {
      machine,
      pos_config,
      production_defaults,
      signing_keys: SigningKeys::default(),
      impersonation: ImpersonationState::default(),
      transaction_pool_policy,
      checkpoint_policy,
      reset_state,
      runtime_state,
      checkpoints: BTreeMap::new(),
      runtime_instance_id: allocate_runtime_instance_id(),
      next_checkpoint_sequence: 1,
    })
  }

  pub(crate) fn create_checkpoint(&mut self) -> Result<CheckpointId, CheckpointLimitError> {
    if self.checkpoints.len() >= self.checkpoint_policy.max_checkpoints {
      return Err(CheckpointLimitError {
        max_checkpoints: self.checkpoint_policy.max_checkpoints,
      });
    }

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

    Ok(checkpoint_id)
  }

  pub(crate) fn revert_to_checkpoint(
    &mut self,
    checkpoint_id: CheckpointId,
  ) -> RevertCheckpointOutcome {
    let next_runtime_state = {
      let Some(checkpoint) = self.resolve_checkpoint(checkpoint_id) else {
        return RevertCheckpointOutcome::Unavailable;
      };

      checkpoint.build_runtime_state(&self.runtime_state, self.transaction_pool_policy)
    };

    // The target and every checkpoint created after it become unavailable.
    let _invalidated_checkpoints = self.checkpoints.split_off(&checkpoint_id.sequence);

    self.runtime_state = next_runtime_state;

    RevertCheckpointOutcome::Reverted
  }

  pub(crate) fn reset(&mut self) {
    let next_runtime_state = self
      .reset_state
      .build_runtime_state(self.transaction_pool_policy, &self.production_defaults);

    self.runtime_state = next_runtime_state;
    self.checkpoints.clear();
    self.impersonation.clear();

    // Keep checkpoint IDs monotonic so cleared IDs are never reused.
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
      .epoch
      .pivot_runtime_block()
      .header()
      .timestamp()
  }

  fn resolve_checkpoint(&self, checkpoint_id: CheckpointId) -> Option<&Checkpoint> {
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
    let block_number = optimistic_head.epoch.pivot_block_number();
    let params = self.machine.params();
    let spec = self.machine.spec(block_number, epoch_height);
    let validation = transaction_validation_context(params, &spec, epoch_height);

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
      let block_number = optimistic_head.epoch.pivot_block_number();
      let params = self.machine.params();
      let spec = self.machine.spec(block_number, epoch_height);
      let validation = transaction_validation_context(params, &spec, epoch_height);

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
    let optimistic_head = self.runtime_state.history.optimistic_head();
    let state = open_committed_state(&optimistic_head.state.version)?;
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
          committed_nonce: state.nonce(&address)?,
          balance: state.balance(&address)?,
        });
      }

      let (sponsored_gas, sponsored_storage) = sponsored_gas_and_storage(&state, transaction)?;
      let transaction_cost = pool_transaction_cost(transaction, sponsored_gas, sponsored_storage);

      transaction_costs.insert(transaction.hash(), transaction_cost);
    }

    Ok(PoolReadinessInputs::new(account_states, transaction_costs))
  }

  pub(crate) fn produce_and_commit_block(
    &mut self,
    header_input: BlockProductionInput,
    block_gas_limit: U256,
  ) -> Result<RuntimeCommitOutcome, RuntimeBlockProductionError> {
    let parent = Arc::clone(self.runtime_state.history.optimistic_head());
    let parent_block = parent.epoch.pivot_runtime_block();

    let prepared_environment = self
      .runtime_state
      .production_environment
      .prepare_next_block(&self.production_defaults, parent_block.header().timestamp())?;

    let epoch_height = parent_block
      .header()
      .height()
      .checked_add(1)
      .expect("a parent block must permit a subsequent epoch height");

    let block_number = parent.epoch.next_epoch_start_block_number();
    let selection_input = self.transaction_pool_selection_input()?;
    let params = self.machine.params();
    let spec = self.machine.spec(block_number, epoch_height);

    let validation = transaction_validation_context(params, &spec, epoch_height);

    let selection = select_transactions(
      &selection_input,
      parent_block,
      params,
      &validation,
      block_gas_limit,
      TransactionSelectionLimits::default(),
    );
    let (block_selection, transactions_to_drop) =
      selection.into_block_selection_and_transactions_to_drop();

    let deferred_commitment = self
      .runtime_state
      .history
      .deferred_commitment_for_header_height(epoch_height);

    let runtime_block = produce_block(
      parent_block,
      block_selection,
      params,
      prepared_environment.timestamp(),
      header_input,
      deferred_commitment,
    );

    let outcome = self.execute_and_commit_single_block_epoch_with_selection_drops(
      runtime_block,
      transactions_to_drop,
      Some(prepared_environment),
    );

    Ok(outcome)
  }

  pub(crate) fn execute_and_commit_single_block_epoch(
    &mut self,
    runtime_block: RuntimeBlock,
  ) -> RuntimeCommitOutcome {
    self.execute_and_commit_single_block_epoch_with_selection_drops(runtime_block, Vec::new(), None)
  }

  fn execute_and_commit_single_block_epoch_with_selection_drops(
    &mut self,
    runtime_block: RuntimeBlock,
    transactions_to_drop: Vec<RuntimeTransaction>,
    prepared_environment: Option<PreparedProductionEnvironment>,
  ) -> RuntimeCommitOutcome {
    let parent = Arc::clone(self.runtime_state.history.optimistic_head());
    let parent_block = parent.epoch.pivot_runtime_block();
    let block_timestamp = runtime_block.header().timestamp();

    assert!(
      block_timestamp >= parent_block.header().timestamp(),
      "block timestamp {block_timestamp} must not be lower than parent timestamp {}",
      parent_block.header().timestamp(),
    );

    if let Some(prepared_environment) = prepared_environment.as_ref() {
      assert_eq!(
        block_timestamp,
        prepared_environment.timestamp(),
        "a produced block must use its prepared timestamp",
      );
    }

    let previous_latest_state_height = self.runtime_state.history.latest_state_height();
    let previous_latest_header_committed_height =
      self.runtime_state.history.latest_header_committed_height();
    let start_block_number = parent.epoch.next_epoch_start_block_number();

    let executed = execute_single_block_epoch(
      self.machine.as_ref(),
      parent_block,
      &parent.state,
      parent.pos_state.as_ref(),
      start_block_number,
      runtime_block,
    );

    let ExecutedSingleBlockEpoch {
      runtime_block,
      state,
      commitment,
      block_receipts,
      transaction_dispositions,
      accounts_for_txpool,
    } = executed;

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

    let next_view = Arc::new(CommittedChainView {
      epoch: CommittedEpoch {
        start_block_number,
        ordered_blocks: vec![runtime_block],
        commitment,
        block_receipts,
      },
      state,
      pos_state: Arc::clone(&parent.pos_state),
    });

    self
      .runtime_state
      .history
      .append_executed_epoch(Arc::clone(&next_view));
    self
      .runtime_state
      .transaction_pool
      .apply_reconciliation(pool_reconciliation);

    match prepared_environment {
      Some(prepared_environment) => self
        .runtime_state
        .production_environment
        .commit_prepared(prepared_environment),
      None => self
        .runtime_state
        .production_environment
        .synchronize_committed_timestamp(block_timestamp),
    }

    let latest_state_advanced_to = (self.runtime_state.history.latest_state_height()
      > previous_latest_state_height)
      .then(|| Arc::clone(self.runtime_state.history.latest_state_view()));
    let latest_header_committed_advanced_to =
      (self.runtime_state.history.latest_header_committed_height()
        > previous_latest_header_committed_height)
        .then(|| Arc::clone(self.runtime_state.history.latest_header_committed_view()));

    RuntimeCommitOutcome {
      optimistic_view: next_view,
      latest_state_advanced_to,
      latest_header_committed_advanced_to,
      transaction_pool_updates: TransactionPoolUpdates {
        transactions_to_repack,
        transactions_dropped_during_selection: transactions_to_drop,
        modified_accounts: accounts_for_txpool,
      },
    }
  }

  pub(crate) fn optimistic_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.runtime_state.history.optimistic_head())
  }

  pub(crate) fn epoch_view_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<Arc<CommittedChainView>> {
    self
      .runtime_state
      .history
      .view_at_epoch_height(epoch_height)
      .map(Arc::clone)
  }

  pub(crate) fn mined_block_by_hash(&self, block_hash: &H256) -> Option<MinedBlockView> {
    self.runtime_state.history.mined_block_by_hash(block_hash)
  }

  pub(crate) fn mined_transaction_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<MinedTransactionView> {
    self
      .runtime_state
      .history
      .mined_transaction_by_hash(transaction_hash)
  }

  pub(crate) fn transaction_receipt_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<TransactionReceiptView> {
    self
      .runtime_state
      .history
      .transaction_receipt_by_hash(transaction_hash)
  }

  pub(crate) fn latest_state_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.runtime_state.history.latest_state_view())
  }

  pub(crate) fn latest_header_committed_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.runtime_state.history.latest_header_committed_view())
  }
}
