//! Immutable chain views and their execution artifacts.

mod history;

pub(crate) use history::CommittedChainHistory;

use std::sync::Arc;

use cfx_statedb::global_params::TOTAL_GLOBAL_PARAMS;
use cfx_types::{AddressWithSpace, H256, U256};
use primitives::{Block, BlockNumber, BlockReceipts, Receipt, block::BlockHeight};

use crate::{
  block_producer::{BlockParent, RuntimeBlock},
  execution::ExecutionCommitment,
  fork::{ForkBase, ForkClient, ForkReadError},
  genesis::{ExecutedGenesis, ExecutedGenesisWithPos},
  pos::{PosContext, PosEnvInput},
  runtime_transaction::RuntimeTransaction,
  state::state_version::{CommittedStateVersion, StateVersion},
};

pub(crate) struct CommittedEpoch {
  start_block_number: BlockNumber,
  ordered_blocks: Vec<RuntimeBlock>,
  commitment: ExecutionCommitment,
  block_receipts: Vec<Arc<BlockReceipts>>,
}
impl CommittedEpoch {
  pub(crate) fn pivot_runtime_block(&self) -> &RuntimeBlock {
    self
      .ordered_blocks
      .last()
      .expect("a committed epoch always contains a pivot block")
  }

  /// Returns the Conflux pivot block without runtime transaction metadata.
  pub(crate) fn pivot_block(&self) -> &Block {
    self.pivot_runtime_block().block()
  }

  pub(crate) fn commitment(&self) -> &ExecutionCommitment {
    &self.commitment
  }

  pub(crate) fn block_receipts(&self) -> &[Arc<BlockReceipts>] {
    &self.block_receipts
  }

  fn pivot_block_number(&self) -> BlockNumber {
    let pivot_index = self
      .ordered_blocks
      .len()
      .checked_sub(1)
      .expect("a committed epoch always contains at least one block");
    let pivot_index =
      BlockNumber::try_from(pivot_index).expect("a committed epoch size must fit in BlockNumber");
    self
      .start_block_number
      .checked_add(pivot_index)
      .expect("the committed pivot block number must fit in BlockNumber")
  }
}
enum ChainEpoch {
  ForkBase(ForkClient),
  Executed(CommittedEpoch),
}

pub(crate) struct CommittedChainView {
  epoch: ChainEpoch,
  state: CommittedStateVersion,
  pos_context: PosContext,
}

impl CommittedChainView {
  pub(crate) fn from_genesis(genesis: ExecutedGenesisWithPos) -> Self {
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
      epoch: ChainEpoch::Executed(CommittedEpoch {
        start_block_number: 0,
        ordered_blocks: vec![RuntimeBlock::from_system_block(block)],
        commitment,
        block_receipts,
      }),
      state: committed_state,
      pos_context: PosContext::Full(Arc::new(committed_pos_state)),
    }
  }

  pub(crate) fn from_fork(client: ForkClient, globals: [U256; TOTAL_GLOBAL_PARAMS]) -> Self {
    let base = client.base();
    let state = CommittedStateVersion {
      epoch_id: base.pivot_hash,
      version: Arc::new(StateVersion::from_fork(client.clone(), globals)),
    };
    let pos_context = PosContext::Fixed {
      reference: base.pos_reference,
      environment: PosEnvInput {
        pos_view: base.pos_view,
        finalized_epoch: base.pivot_decision.height,
      },
    };

    Self {
      epoch: ChainEpoch::ForkBase(client),
      state,
      pos_context,
    }
  }

  pub(crate) fn from_executed_block(
    parent: &Self,
    runtime_block: RuntimeBlock,
    state: CommittedStateVersion,
    commitment: ExecutionCommitment,
    block_receipts: Vec<Arc<BlockReceipts>>,
  ) -> Self {
    Self {
      epoch: ChainEpoch::Executed(CommittedEpoch {
        start_block_number: parent.next_epoch_start_block_number(),
        ordered_blocks: vec![runtime_block],
        commitment,
        block_receipts,
      }),
      state,
      pos_context: parent.pos_context.clone(),
    }
  }

  /// The remote base has state and parent inputs, but no local execution artifacts.
  pub(crate) fn executed_epoch(&self) -> Option<&CommittedEpoch> {
    match &self.epoch {
      ChainEpoch::Executed(epoch) => Some(epoch),
      ChainEpoch::ForkBase(_) => None,
    }
  }

  fn local_epoch(&self) -> &CommittedEpoch {
    self
      .executed_epoch()
      .expect("local history indexes must reference a locally executed epoch")
  }

  pub(crate) fn epoch_height(&self) -> BlockHeight {
    match &self.epoch {
      ChainEpoch::ForkBase(client) => client.base().epoch_height,
      ChainEpoch::Executed(epoch) => epoch.pivot_runtime_block().header().height(),
    }
  }

  pub(crate) fn execution_parent(&self) -> BlockParent {
    match &self.epoch {
      ChainEpoch::ForkBase(client) => {
        let base = client.base();
        BlockParent {
          hash: base.pivot_hash,
          height: base.epoch_height,
          timestamp: base.timestamp,
          author: base.author,
          gas_limit: base.header_gas_limit,
          base_price: base.base_price,
          pos_reference: base.pos_reference,
        }
      }
      ChainEpoch::Executed(epoch) => BlockParent::from_header(epoch.pivot_runtime_block().header()),
    }
  }

  pub(crate) fn pivot_block_number(&self) -> BlockNumber {
    match &self.epoch {
      ChainEpoch::ForkBase(client) => client.base().pivot_block_number,
      ChainEpoch::Executed(epoch) => epoch.pivot_block_number(),
    }
  }

  pub(crate) fn next_epoch_start_block_number(&self) -> BlockNumber {
    self
      .pivot_block_number()
      .checked_add(1)
      .expect("a committed parent must permit a subsequent block number")
  }

  pub(crate) fn state(&self) -> &CommittedStateVersion {
    &self.state
  }

  pub(crate) fn pos_context(&self) -> &PosContext {
    &self.pos_context
  }

  pub(crate) fn fork_base(&self) -> Option<&ForkBase> {
    match &self.epoch {
      ChainEpoch::ForkBase(client) => Some(client.base()),
      ChainEpoch::Executed(_) => None,
    }
  }
}

/// History can be retained locally or read from the fixed remote baseline.
/// Only locally executed history includes a local state version and artifacts.
#[derive(Clone)]
pub(crate) enum EpochHistoryView {
  Local(Arc<CommittedChainView>),
  Fork {
    client: ForkClient,
    epoch_height: BlockHeight,
  },
}

impl EpochHistoryView {
  pub(crate) fn epoch_height(&self) -> BlockHeight {
    match self {
      Self::Local(view) => view.epoch_height(),
      Self::Fork { epoch_height, .. } => *epoch_height,
    }
  }

  pub(crate) fn commitment(&self) -> Result<ExecutionCommitment, ForkReadError> {
    match self {
      Self::Local(view) => Ok(view.local_epoch().commitment().clone()),
      Self::Fork {
        client,
        epoch_height,
      } => client.commitment(*epoch_height),
    }
  }
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
      .local_epoch()
      .ordered_blocks
      .get(self.block_index)
      .expect("an indexed block must exist in its committed chain view")
  }

  /// Returns the Conflux block without runtime transaction metadata.
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
    self.chain_view.epoch_height()
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
    Self::block_receipts_for(&transaction)
      .receipts
      .get(transaction.transaction_index)?;

    Some(Self { transaction })
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
      .local_epoch()
      .block_receipts
      .get(transaction.block.block_index)
      .expect("an indexed block must have committed receipts")
  }
}
