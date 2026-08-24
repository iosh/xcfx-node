//! Owns one node instance's authoritative protocol state.
use std::{
  collections::{BTreeMap, HashMap},
  sync::Arc,
};

use cfx_executor::{machine::Machine, spec::CommonParams, state::State};
use cfx_statedb::{Result as StateResult, StateDb};
use cfx_types::{
  AddressSpaceUtil, AddressWithSpace, AllChainID, H256, Space, U256,
  address_util::AddressUtil,
};
use cfx_vm_types::Spec;
use diem_types::term_state::pos_state_config::PosStateConfig;

use crate::{
  block_producer::{BlockProductionInput, produce_block},
  execution::{
    ExecutedSingleBlockEpoch, TransactionExecutionDisposition, execute_single_block_epoch,
  },
  genesis::{
    ExecutedGenesis, ExecutedGenesisWithPos, GenesisError, GenesisHeaderInput,
    execute_genesis_with_pos,
  },
  pos::{CommittedPosState, GenesisPosDefinition},
  state::{
    layered_mpt_state::LayeredMptState,
    state_version::{CommittedStateVersion, StateCandidate, StateVersion},
  },
  transaction_ingress::{TransactionValidationContext, decode_and_validate_raw_transaction},
  transaction_pool::{
    AccountKey, PoolAccountState, PoolEntryStates, PoolReadinessInputs, PoolSelectionInput,
    TransactionPool, TransactionPoolInsertOutcome, TransactionPoolPolicy, TransactionPoolView,
    pool_gas_cost, pool_transaction_cost,
  },
  transaction_selector::{TransactionSelectionLimits, select_transactions},
};

use cfx_internal_common::EpochExecutionCommitment;
use cfx_parameters::{
  consensus::{DEFERRED_STATE_EPOCH_COUNT, TRANSACTION_DEFAULT_EPOCH_BOUND},
  staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT,
};

use primitives::{
  Account, Action, Block, BlockNumber, BlockReceipts, Receipt, SignedTransaction, Transaction,
  block::BlockHeight, transaction::TransactionError,
};

pub(crate) struct CommittedEpoch {
  start_block_number: BlockNumber,
  ordered_blocks: Vec<Block>,
  commitment: EpochExecutionCommitment,
  block_receipts: Vec<Arc<BlockReceipts>>,
}

impl CommittedEpoch {
  pub(crate) fn pivot_block(&self) -> &Block {
    self
      .ordered_blocks
      .last()
      .expect("a committed epoch always contains a pivot block")
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
        ordered_blocks: vec![block],
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
  pub(crate) fn block(&self) -> &Block {
    self
      .chain_view
      .epoch
      .ordered_blocks
      .get(self.block_index)
      .expect("an indexed block must exist in its committed chain view")
  }

  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self.chain_view.epoch.pivot_block().block_header.height()
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

  pub(crate) fn transaction(&self) -> &Arc<SignedTransaction> {
    self
      .block
      .block()
      .transactions
      .get(self.transaction_index)
      .expect("an indexed transaction must exist in its mined block")
  }

  pub(crate) fn transaction_index(&self) -> usize {
    self.transaction_index
  }
}

#[derive(Clone)]
pub(crate) struct TransactionReceiptView {
  transaction: MinedTransactionView,
}

impl TransactionReceiptView {
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
    self
      .transaction
      .block
      .chain_view
      .epoch
      .block_receipts
      .get(self.transaction.block.block_index)
      .expect("an indexed block must have committed receipts")
  }
}

struct CommittedChainHistory {
  views: Vec<Arc<CommittedChainView>>,
  block_locations: HashMap<H256, BlockLocation>,
  transaction_locations: HashMap<H256, TransactionLocation>,
}

impl CommittedChainHistory {
  fn from_genesis(genesis: CommittedChainView) -> Self {
    let mut history = Self {
      views: Vec::new(),
      block_locations: HashMap::new(),
      transaction_locations: HashMap::new(),
    };

    history.append(Arc::new(genesis));
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
      .pivot_block()
      .block_header
      .height()
  }

  fn append(&mut self, view: Arc<CommittedChainView>) {
    let epoch_height = view.epoch.pivot_block().block_header.height();
    let expected_epoch_height = BlockHeight::try_from(self.views.len())
      .expect("a linear history length must fit in BlockHeight");

    assert_eq!(
      epoch_height, expected_epoch_height,
      "a linear history append must contain the next epoch height",
    );
    assert_eq!(
      view.epoch.ordered_blocks.len(),
      view.epoch.block_receipts.len(),
      "every committed block must have one block receipt collection",
    );

    let mut block_locations = HashMap::with_capacity(view.epoch.ordered_blocks.len());
    let mut transaction_locations = HashMap::new();

    for (block_index, (block, block_receipts)) in view
      .epoch
      .ordered_blocks
      .iter()
      .zip(&view.epoch.block_receipts)
      .enumerate()
    {
      assert_eq!(
        block.transactions.len(),
        block_receipts.receipts.len(),
        "every block transaction must have one execution receipt",
      );
      assert_eq!(
        block.transactions.len(),
        block_receipts.tx_execution_error_messages.len(),
        "every block transaction must have one execution error entry",
      );

      let block_hash = block.hash();
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

      for (transaction_index, (transaction, receipt)) in block
        .transactions
        .iter()
        .zip(&block_receipts.receipts)
        .enumerate()
      {
        if receipt.tx_skipped() {
          continue;
        }

        let transaction_hash = transaction.hash();
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

    if view.epoch.pivot_block().block_header.height() == epoch_height {
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

  fn contains_executed_transaction(&self, transaction_hash: &H256) -> bool {
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
      .block()
      .transactions
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

    Some(TransactionReceiptView {
      transaction: self.mined_transaction_at(location),
    })
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
  pub(crate) transactions_to_repack: Vec<Arc<SignedTransaction>>,
  pub(crate) transactions_dropped_during_selection: Vec<Arc<SignedTransaction>>,
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
  transaction: &SignedTransaction,
) -> StateResult<(U256, u64)> {
  let Transaction::Native(native_transaction) = &transaction.unsigned else {
    return Ok(Default::default());
  };

  let contract_address = match native_transaction.action() {
    Action::Call(address) if address.is_contract_address() => address,
    _ => return Ok(Default::default()),
  };

  let Some(sponsor_info) = state.sponsor_info(&contract_address)? else {
    return Ok(Default::default());
  };

  if !state.check_contract_whitelist(&contract_address, &transaction.sender)? {
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

pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  pos_config: PosStateConfig,
  history: CommittedChainHistory,
  transaction_pool: TransactionPool,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    machine: Arc<Machine>,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
    pos_config: PosStateConfig,
    transaction_pool_policy: TransactionPoolPolicy,
  ) -> Result<Self, GenesisError> {
    let genesis = execute_genesis_with_pos(
      Arc::clone(&machine),
      allocations,
      header,
      &pos_definition,
      &pos_config,
    )?;

    Ok(Self {
      machine,
      pos_config,
      history: CommittedChainHistory::from_genesis(CommittedChainView::from_genesis(genesis)),
      transaction_pool: TransactionPool::new(transaction_pool_policy),
    })
  }

  pub(crate) fn transaction_pool_selection_input(&self) -> StateResult<PoolSelectionInput> {
    let view = self.transaction_pool_view();
    let inputs = self.pool_readiness_inputs(&view)?;
    let entry_states = self.derive_transaction_pool_states(&inputs);

    Ok(PoolSelectionInput { view, entry_states })
  }

  /// Validates raw transaction bytes against the current optimistic view and
  /// inserts the recovered transaction into this Runtime's pool.
  pub(crate) fn submit_raw_transaction(&mut self, raw: &[u8]) -> Result<H256, TransactionError> {
    let transaction = {
      let optimistic_head = self.history.optimistic_head();
      let epoch_height = self.history.optimistic_height();
      let block_number = optimistic_head.epoch.pivot_block_number();
      let params = self.machine.params();
      let spec = self.machine.spec(block_number, epoch_height);
      let validation = transaction_validation_context(params, &spec, epoch_height);

      Arc::new(decode_and_validate_raw_transaction(raw, &validation)?)
    };

    let transaction_hash = transaction.hash();
    self.insert_admitted_transaction(transaction)?;

    Ok(transaction_hash)
  }

  /// Inserts a transaction that has already passed transaction-only ingress validation.
  pub(crate) fn insert_admitted_transaction(
    &mut self,
    transaction: Arc<SignedTransaction>,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    if self
      .history
      .contains_executed_transaction(&transaction.hash())
    {
      return Err(TransactionError::AlreadyImported);
    }

    self.transaction_pool.insert(transaction)
  }

  /// Returns a stable snapshot of the transaction pool.
  pub(crate) fn transaction_pool_view(&self) -> TransactionPoolView {
    self.transaction_pool.view()
  }

  pub(crate) fn derive_transaction_pool_states(
    &self,
    inputs: &PoolReadinessInputs,
  ) -> PoolEntryStates {
    self.transaction_pool.derive_entry_states(inputs)
  }

  pub(crate) fn remove_stale_transactions(
    &mut self,
    entry_states: &PoolEntryStates,
  ) -> Vec<Arc<SignedTransaction>> {
    self.transaction_pool.remove_stale(entry_states)
  }

  pub(crate) fn pool_readiness_inputs(
    &self,
    pool_view: &TransactionPoolView,
  ) -> StateResult<PoolReadinessInputs> {
    let optimistic_head = self.history.optimistic_head();
    let state = open_committed_state(&optimistic_head.state.version)?;
    let mut account_states = BTreeMap::<AccountKey, PoolAccountState>::new();
    let mut transaction_costs = BTreeMap::new();

    for entry in &pool_view.entries {
      let transaction = entry.transaction.as_ref();
      let account_key = (transaction.sender, transaction.space());

      if !account_states.contains_key(&account_key) {
        let address = transaction.sender.with_space(transaction.space());

        account_states.insert(
          account_key,
          PoolAccountState {
            committed_nonce: state.nonce(&address)?,
            balance: state.balance(&address)?,
          },
        );
      }

      let (sponsored_gas, sponsored_storage) =
        sponsored_gas_and_storage(&state, transaction)?;
      let transaction_cost =
        pool_transaction_cost(transaction, sponsored_gas, sponsored_storage);

      transaction_costs.insert(transaction.hash(), transaction_cost);
    }

    Ok(PoolReadinessInputs::new(account_states, transaction_costs))
  }

  pub(crate) fn produce_and_commit_block(
    &mut self,
    header_input: BlockProductionInput,
    block_gas_limit: U256,
  ) -> StateResult<RuntimeCommitOutcome> {
    let parent = Arc::clone(self.history.optimistic_head());
    let parent_block = parent.epoch.pivot_block();

    let epoch_height = parent_block
      .block_header
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
      .history
      .deferred_commitment_for_header_height(epoch_height);

    let block = produce_block(
      parent_block,
      block_selection,
      params,
      header_input,
      deferred_commitment,
    );

    Ok(self.execute_and_commit_single_block_epoch_with_selection_drops(
      block,
      transactions_to_drop,
    ))
  }

  pub(crate) fn execute_and_commit_single_block_epoch(
    &mut self,
    block: Block,
  ) -> RuntimeCommitOutcome {
    self.execute_and_commit_single_block_epoch_with_selection_drops(block, Vec::new())
  }

  fn execute_and_commit_single_block_epoch_with_selection_drops(
    &mut self,
    block: Block,
    transactions_to_drop: Vec<Arc<SignedTransaction>>,
  ) -> RuntimeCommitOutcome {
    let parent = Arc::clone(self.history.optimistic_head());
    let previous_latest_state_height = self.history.latest_state_height();
    let previous_latest_header_committed_height = self.history.latest_header_committed_height();
    let start_block_number = parent.epoch.next_epoch_start_block_number();

    let executed = execute_single_block_epoch(
      self.machine.as_ref(),
      parent.epoch.pivot_block(),
      &parent.state,
      parent.pos_state.as_ref(),
      start_block_number,
      block,
    );

    let ExecutedSingleBlockEpoch {
      block,
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

    for (transaction, disposition) in block.transactions.iter().zip(transaction_dispositions) {
      match disposition {
        TransactionExecutionDisposition::Executed
        | TransactionExecutionDisposition::SkippedDrop => {
          transaction_hashes_to_remove.push(transaction.hash());
        }
        TransactionExecutionDisposition::SkippedRepack => {
          transactions_to_repack.push(Arc::clone(transaction));
        }
      }
    }

    let pool_reconciliation = self
      .transaction_pool
      .prepare_reconciliation(transaction_hashes_to_remove, &accounts_for_txpool);

    let next_view = Arc::new(CommittedChainView {
      epoch: CommittedEpoch {
        start_block_number,
        ordered_blocks: vec![block],
        commitment,
        block_receipts,
      },
      state,
      pos_state: Arc::clone(&parent.pos_state),
    });

    self.history.append(Arc::clone(&next_view));
    self
      .transaction_pool
      .apply_reconciliation(pool_reconciliation);

    let latest_state_advanced_to = (self.history.latest_state_height()
      > previous_latest_state_height)
      .then(|| Arc::clone(self.history.latest_state_view()));
    let latest_header_committed_advanced_to = (self.history.latest_header_committed_height()
      > previous_latest_header_committed_height)
      .then(|| Arc::clone(self.history.latest_header_committed_view()));

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
    Arc::clone(self.history.optimistic_head())
  }

  pub(crate) fn epoch_view_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<Arc<CommittedChainView>> {
    self
      .history
      .view_at_epoch_height(epoch_height)
      .map(Arc::clone)
  }

  pub(crate) fn mined_block_by_hash(&self, block_hash: &H256) -> Option<MinedBlockView> {
    self.history.mined_block_by_hash(block_hash)
  }

  pub(crate) fn mined_transaction_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<MinedTransactionView> {
    self.history.mined_transaction_by_hash(transaction_hash)
  }

  pub(crate) fn transaction_receipt_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<TransactionReceiptView> {
    self.history.transaction_receipt_by_hash(transaction_hash)
  }

  pub(crate) fn latest_state_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.history.latest_state_view())
  }

  pub(crate) fn latest_header_committed_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.history.latest_header_committed_view())
  }
}
