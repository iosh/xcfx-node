//! Owns one node instance's authoritative protocol state.
use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::machine::Machine;
use cfx_types::{AddressWithSpace, U256};
use diem_types::term_state::pos_state_config::PosStateConfig;

use crate::{
  execution::{ExecutedSingleBlockEpoch, execute_single_block_epoch},
  genesis::{
    ExecutedGenesis, ExecutedGenesisWithPos, GenesisError, GenesisHeaderInput,
    execute_genesis_with_pos,
  },
  pos::{CommittedPosState, GenesisPosDefinition},
  state::state_version::CommittedStateVersion,
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

pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  pos_config: PosStateConfig,
  current: Arc<CommittedChainView>,
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
    })
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
