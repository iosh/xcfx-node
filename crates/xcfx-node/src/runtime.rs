//! Owns one node instance's authoritative protocol state.
use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::{machine::Machine, state::State};
use cfx_statedb::{Result as StateResult, StateDb};
use cfx_types::{AddressSpaceUtil, AddressWithSpace, U256};
use diem_types::term_state::pos_state_config::PosStateConfig;

use crate::{
  execution::{ExecutedSingleBlockEpoch, execute_single_block_epoch},
  genesis::{
    ExecutedGenesis, ExecutedGenesisWithPos, GenesisError, GenesisHeaderInput,
    execute_genesis_with_pos,
  },
  pos::{CommittedPosState, GenesisPosDefinition},
  state::{
    layered_mpt_state::LayeredMptState,
    state_version::{CommittedStateVersion, StateCandidate, StateVersion},
  },
  transaction_pool::{
    AccountKey, PoolAccountState, PoolEntryStates, PoolReadinessInputs, PoolSelectionInput,
    TransactionPool, TransactionPoolInsertOutcome, TransactionPoolPolicy, TransactionPoolView,
    pool_transaction_cost,
  },
};

use cfx_internal_common::EpochExecutionCommitment;
use primitives::{Account, Block, BlockNumber, BlockReceipts, SignedTransaction};

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

pub(crate) struct TransactionPoolUpdates {
  pub(crate) transactions_to_repack: Vec<Arc<SignedTransaction>>,
  pub(crate) modified_accounts: Vec<Account>,
}

fn open_committed_state(version: &Arc<StateVersion>) -> StateResult<State> {
  let (backend, _) = LayeredMptState::new(StateCandidate::new(Arc::clone(version)));
  State::new(StateDb::new(Box::new(backend)))
}

pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  pos_config: PosStateConfig,
  current: Arc<CommittedChainView>,
  transaction_pool: TransactionPool,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    machine: Arc<Machine>,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
    pos_config: PosStateConfig,
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
      current: Arc::new(CommittedChainView::from_genesis(genesis)),
      transaction_pool: TransactionPool::new(),
    })
  }

  pub(crate) fn transaction_pool_selection_input(&self) -> StateResult<PoolSelectionInput> {
    let view = self.transaction_pool_view();
    let inputs = self.pool_readiness_inputs(&view)?;
    let entry_states = self.derive_transaction_pool_states(&inputs);

    Ok(PoolSelectionInput { view, entry_states })
  }

  /// Inserts a transaction that has already passed transaction-only ingress validation.
  pub(crate) fn insert_admitted_transaction(
    &mut self,
    transaction: Arc<SignedTransaction>,
    policy: &TransactionPoolPolicy,
  ) -> Result<TransactionPoolInsertOutcome, primitives::transaction::TransactionError> {
    self.transaction_pool.insert(transaction, policy)
  }

  /// Returns the pool entries associated with the current committed chain view.
  pub(crate) fn transaction_pool_view(&self) -> TransactionPoolView {
    self
      .transaction_pool
      .view(self.current.epoch().pivot_block().hash())
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
    let mut state = open_committed_state(&self.current.state.version)?;

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

      transaction_costs.insert(
        transaction.hash(),
        pool_transaction_cost(transaction, U256::zero(), 0),
      );
    }

    Ok(PoolReadinessInputs::new(account_states, transaction_costs))
  }

  pub(crate) fn execute_and_commit_single_block_epoch(
    &mut self,
    block: Block,
  ) -> TransactionPoolUpdates {
    let parent = Arc::clone(&self.current);
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
      transactions_to_repack,
      accounts_for_txpool,
    } = executed;

    let next_view = CommittedChainView {
      epoch: CommittedEpoch {
        start_block_number,
        ordered_blocks: vec![block],
        commitment,
        block_receipts,
      },
      state,
      pos_state: Arc::clone(&parent.pos_state),
    };

    let transaction_pool_updates = TransactionPoolUpdates {
      transactions_to_repack,
      modified_accounts: accounts_for_txpool,
    };

    self.current = Arc::new(next_view);

    transaction_pool_updates
  }

  pub(crate) fn current_view(&self) -> Arc<CommittedChainView> {
    Arc::clone(&self.current)
  }
}
