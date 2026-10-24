//! Immutable epoch views, retained execution artifacts, and indexed queries.

mod espace;
mod graph;
mod history;

pub(crate) use espace::{
  EspaceBlockView, EspaceReceiptRef, EspaceTransactionRef, EspaceTransactionView,
};
pub(crate) use graph::{BlockGraph, GraphError, OrderedEpoch};
pub(crate) use history::ChainHistory;

use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::machine::Machine;
use cfx_statedb::global_params::TOTAL_GLOBAL_PARAMS;
use cfx_types::{AddressWithSpace, H256, Space, U256};
use primitives::{Block, BlockNumber, BlockReceipts, Receipt, block::BlockHeight};

use espace::EspaceProjection;

use crate::{
  block_producer::{BlockParent, RuntimeBlock},
  execution::{ExecutedBlock, ExecutedEpoch, ExecutionCommitment},
  fork::{ForkBase, ForkClient, ForkReadError},
  genesis::ExecutedGenesisWithPos,
  pos::{PosContext, PosEnvInput},
  runtime_transaction::RuntimeTransaction,
  state::{
    balance::{BalanceChangeError, prepare_initial_balances},
    state_version::{CommittedStateVersion, StateVersion},
  },
};

/// Blocks, receipts, and execution commitments retained for one local epoch.
pub(crate) struct EpochArtifacts {
  start_block_number: BlockNumber,
  ordered_blocks: Vec<EpochBlock>,
  skipped_blocks: Vec<EpochBlock>,
  commitment: ExecutionCommitment,
  espace: EspaceProjection,
  has_state_controls: bool,
}

/// A block and its results in one retained epoch, including Genesis and skipped blocks.
pub(crate) struct EpochBlock {
  block: RuntimeBlock,
  receipts: Option<Arc<BlockReceipts>>,
  core_transaction_indices: Vec<usize>,
}

/// A position in the ordered block bodies of one local epoch.
#[derive(Clone, Copy)]
struct EpochTransactionIndex {
  block_index: usize,
  transaction_index: usize,
}

/// Borrows an original transaction, receipt, and error at the same block-body position.
#[derive(Clone, Copy)]
pub(crate) struct TransactionReceiptRef<'a> {
  block: &'a RuntimeBlock,
  receipts: &'a BlockReceipts,
  index: usize,
}

impl EpochBlock {
  fn new(block: RuntimeBlock, receipts: Option<Arc<BlockReceipts>>) -> Self {
    let core_transaction_indices = block
      .transactions()
      .iter()
      .enumerate()
      .filter_map(|(index, transaction)| (transaction.space() == Space::Native).then_some(index))
      .collect();
    Self {
      block,
      receipts,
      core_transaction_indices,
    }
  }

  fn from_executed(executed: ExecutedBlock) -> Self {
    let (block, receipts) = executed.into_parts();
    Self::new(block, Some(receipts))
  }

  fn from_skipped(block: RuntimeBlock) -> Self {
    Self::new(block, None)
  }

  pub(crate) fn runtime_block(&self) -> &RuntimeBlock {
    &self.block
  }

  /// Genesis has an empty collection; a skipped block has none.
  pub(crate) fn block_receipts(&self) -> Option<&BlockReceipts> {
    self.receipts.as_deref()
  }

  /// Includes skipped transaction outcomes; Genesis and skipped blocks yield no receipts.
  pub(crate) fn transaction_receipts(&self) -> impl Iterator<Item = TransactionReceiptRef<'_>> {
    self.receipts.iter().flat_map(move |receipts| {
      (0..receipts.receipts.len()).map(move |index| TransactionReceiptRef {
        block: &self.block,
        receipts,
        index,
      })
    })
  }

  fn transaction_receipt_at(&self, index: usize) -> Option<TransactionReceiptRef<'_>> {
    let receipts = self.receipts.as_deref()?;
    receipts.receipts.get(index)?;
    Some(TransactionReceiptRef {
      block: &self.block,
      receipts,
      index,
    })
  }
}

impl<'a> TransactionReceiptRef<'a> {
  pub(crate) fn transaction(&self) -> &'a RuntimeTransaction {
    &self.block.transactions()[self.index]
  }

  /// Position in the original mixed-Space block body.
  pub(crate) fn block_transaction_index(&self) -> usize {
    self.index
  }

  pub(crate) fn receipt(&self) -> &'a Receipt {
    &self.receipts.receipts[self.index]
  }

  pub(crate) fn execution_error_message(&self) -> Option<&'a str> {
    let error = &self.receipts.tx_execution_error_messages[self.index];
    (!error.is_empty()).then_some(error.as_str())
  }
}

impl EpochArtifacts {
  pub(crate) fn ordered_blocks(&self) -> &[EpochBlock] {
    &self.ordered_blocks
  }

  pub(crate) fn skipped_blocks(&self) -> &[EpochBlock] {
    &self.skipped_blocks
  }

  pub(crate) fn all_blocks(&self) -> impl Iterator<Item = &RuntimeBlock> {
    self
      .skipped_blocks
      .iter()
      .chain(&self.ordered_blocks)
      .map(EpochBlock::runtime_block)
  }

  pub(crate) fn pivot_runtime_block(&self) -> &RuntimeBlock {
    self
      .ordered_blocks
      .last()
      .expect("a committed epoch always contains a pivot block")
      .runtime_block()
  }

  /// Returns the standard pivot of a test fixture.
  #[cfg(test)]
  pub(crate) fn pivot_block(&self) -> &Block {
    self
      .pivot_runtime_block()
      .standard_block()
      .expect("a standard-block test fixture must retain a standard pivot")
  }

  pub(crate) fn commitment(&self) -> &ExecutionCommitment {
    &self.commitment
  }

  fn transaction(&self, index: EpochTransactionIndex) -> &RuntimeTransaction {
    self
      .ordered_blocks
      .get(index.block_index)
      .expect("a transaction position must reference an ordered block in its epoch")
      .runtime_block()
      .transactions()
      .get(index.transaction_index)
      .expect("a transaction position must reference a transaction in its block")
  }

  fn transaction_receipt(&self, index: EpochTransactionIndex) -> TransactionReceiptRef<'_> {
    self.ordered_blocks[index.block_index]
      .transaction_receipt_at(index.transaction_index)
      .expect("a projected transaction must retain its original receipt")
  }

  fn espace_transactions(&self) -> impl Iterator<Item = EspaceTransactionRef<'_>> + '_ {
    self.espace.transactions(self)
  }

  /// Whether this epoch committed developer state controls, including no-op controls.
  pub(crate) fn has_state_controls(&self) -> bool {
    self.has_state_controls
  }

  /// Includes failed executions with receipts, but excludes skipped transactions.
  pub(crate) fn has_executed_transactions(&self) -> bool {
    self
      .ordered_blocks
      .iter()
      .flat_map(EpochBlock::transaction_receipts)
      .any(|result| !result.receipt().tx_skipped())
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
enum EpochSource {
  ForkBase(ForkClient),
  Executed(EpochArtifacts),
}

/// Immutable state and execution context for one retained epoch.
///
/// A Fork origin carries remote parent metadata without local execution artifacts.
pub(crate) struct EpochView {
  source: EpochSource,
  state: CommittedStateVersion,
  pos_context: PosContext,
}

impl EpochView {
  pub(crate) fn from_genesis(genesis: ExecutedGenesisWithPos) -> Self {
    let ExecutedGenesisWithPos {
      execution,
      committed_pos_state,
    } = genesis;

    let (block, committed_state, commitment, receipts) = execution.into_parts();
    let block = EpochBlock::new(RuntimeBlock::from_system_block(block), Some(receipts));

    Self {
      source: EpochSource::Executed(EpochArtifacts {
        start_block_number: 0,
        ordered_blocks: vec![block],
        skipped_blocks: Vec::new(),
        commitment,
        espace: EspaceProjection::default(),
        has_state_controls: false,
      }),
      state: committed_state,
      pos_context: PosContext::Full(Arc::new(committed_pos_state)),
    }
  }

  /// Retains startup funding in the initial view, including the view used by reset.
  /// May read remote account state synchronously; the calling thread must allow blocking.
  ///
  /// # Errors
  /// Returns balance access or supply errors without creating a view.
  pub(crate) fn from_fork(
    client: ForkClient,
    globals: [U256; TOTAL_GLOBAL_PARAMS],
    allocations: BTreeMap<AddressWithSpace, U256>,
  ) -> Result<Self, BalanceChangeError> {
    let base = client.base();
    let state = CommittedStateVersion {
      epoch_id: base.pivot_hash,
      version: prepare_initial_balances(
        Arc::new(StateVersion::from_fork(client.clone(), globals)),
        allocations,
      )?,
    };
    let pos_context = PosContext::Fixed {
      reference: base.pos_reference,
      environment: PosEnvInput {
        pos_view: base.pos_view,
        finalized_epoch: base.pivot_decision.height,
      },
    };

    Ok(Self {
      source: EpochSource::ForkBase(client),
      state,
      pos_context,
    })
  }

  pub(crate) fn from_executed_epoch(
    machine: &Machine,
    executed: ExecutedEpoch,
    skipped_blocks: Vec<RuntimeBlock>,
    has_state_controls: bool,
  ) -> Self {
    let epoch_height = executed.pivot_block().header().height();
    let (start_block_number, blocks, state, commitment, pos_context) = executed.into_parts();
    let ordered_blocks: Vec<_> = blocks.into_iter().map(EpochBlock::from_executed).collect();
    let skipped_blocks = skipped_blocks
      .into_iter()
      .map(EpochBlock::from_skipped)
      .collect();
    let espace = EspaceProjection::from_executed_epoch(
      machine,
      start_block_number,
      epoch_height,
      &ordered_blocks,
    );

    Self {
      source: EpochSource::Executed(EpochArtifacts {
        start_block_number,
        ordered_blocks,
        skipped_blocks,
        commitment,
        espace,
        has_state_controls,
      }),
      state,
      pos_context,
    }
  }

  /// The remote base has state and parent inputs, but no local execution artifacts.
  pub(crate) fn artifacts(&self) -> Option<&EpochArtifacts> {
    match &self.source {
      EpochSource::Executed(artifacts) => Some(artifacts),
      EpochSource::ForkBase(_) => None,
    }
  }

  /// Pins the local eSpace projection. A Fork Base has no local block artifacts.
  /// Availability here does not establish visibility for a standard RPC method.
  pub(crate) fn espace_block(self: &Arc<Self>) -> Option<EspaceBlockView> {
    EspaceBlockView::new(Arc::clone(self))
  }

  fn local_artifacts(&self) -> &EpochArtifacts {
    self
      .artifacts()
      .expect("local history indexes must reference a locally executed epoch")
  }

  pub(crate) fn epoch_height(&self) -> BlockHeight {
    match &self.source {
      EpochSource::ForkBase(client) => client.base().epoch_height,
      EpochSource::Executed(artifacts) => artifacts.pivot_runtime_block().header().height(),
    }
  }

  pub(crate) fn execution_parent(&self) -> BlockParent {
    match &self.source {
      EpochSource::ForkBase(client) => {
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
      EpochSource::Executed(artifacts) => {
        BlockParent::from_header(artifacts.pivot_runtime_block().header())
      }
    }
  }

  pub(crate) fn pivot_block_number(&self) -> BlockNumber {
    match &self.source {
      EpochSource::ForkBase(client) => client.base().pivot_block_number,
      EpochSource::Executed(artifacts) => artifacts.pivot_block_number(),
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
    self.fork_client().map(ForkClient::base)
  }

  pub(crate) fn fork_client(&self) -> Option<&ForkClient> {
    match &self.source {
      EpochSource::ForkBase(client) => Some(client),
      EpochSource::Executed(_) => None,
    }
  }
}

/// History can be retained locally or read from the fixed remote baseline.
/// Only locally executed history includes a local state version and artifacts.
#[derive(Clone)]
pub(crate) enum EpochHistoryView {
  Local(Arc<EpochView>),
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
      Self::Local(view) => Ok(view.local_artifacts().commitment().clone()),
      Self::Fork {
        client,
        epoch_height,
      } => client.commitment(*epoch_height),
    }
  }
}

#[derive(Clone)]
pub(crate) struct MinedBlockView {
  epoch_view: Arc<EpochView>,
  block_index: BlockIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockIndex {
  Ordered(usize),
  Skipped(usize),
}

impl MinedBlockView {
  fn epoch_block(&self) -> &EpochBlock {
    let artifacts = self.epoch_view.local_artifacts();
    let block = match self.block_index {
      BlockIndex::Ordered(index) => artifacts.ordered_blocks.get(index),
      BlockIndex::Skipped(index) => artifacts.skipped_blocks.get(index),
    };
    block.expect("an indexed block must exist in its retained epoch view")
  }

  pub(crate) fn runtime_block(&self) -> &RuntimeBlock {
    self.epoch_block().runtime_block()
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

  fn core_transaction_indices(&self) -> &[usize] {
    &self.epoch_block().core_transaction_indices
  }

  /// Native block-body order, including transactions skipped during execution.
  pub(crate) fn core_transactions(
    &self,
  ) -> impl ExactSizeIterator<Item = MinedTransactionView> + '_ {
    self
      .core_transaction_indices()
      .iter()
      .map(move |&transaction_index| MinedTransactionView {
        block: self.clone(),
        transaction_index,
      })
  }

  pub(crate) fn core_transaction_at(&self, core_index: usize) -> Option<MinedTransactionView> {
    let transaction_index = *self.core_transaction_indices().get(core_index)?;
    Some(MinedTransactionView {
      block: self.clone(),
      transaction_index,
    })
  }

  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self.epoch_view.epoch_height()
  }

  /// Pivot hash of the epoch containing this block in the retained view.
  pub(crate) fn epoch_id(&self) -> H256 {
    self.epoch_view.state().epoch_id
  }

  /// Core execution-order number; skipped blocks have no number.
  pub(crate) fn block_number(&self) -> Option<BlockNumber> {
    match self.block_index {
      BlockIndex::Ordered(index) => {
        Some(self.epoch_view.local_artifacts().start_block_number + index as u64)
      }
      BlockIndex::Skipped(_) => None,
    }
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
      .runtime_block()
      .transactions()
      .get(self.transaction_index)
      .expect("an indexed transaction must exist in its mined block")
  }

  /// Position in the original block body, which contains both Spaces.
  pub(crate) fn block_transaction_index(&self) -> usize {
    self.transaction_index
  }

  /// Native-only block position; an eSpace transaction has no Core index.
  pub(crate) fn core_transaction_index(&self) -> Option<usize> {
    self
      .block
      .core_transaction_indices()
      .binary_search(&self.transaction_index)
      .ok()
  }

  /// Retained execution result without a current-history visibility check.
  /// Genesis transactions and transactions in skipped blocks have no receipts.
  pub(crate) fn receipt(&self) -> Option<TransactionReceiptView> {
    TransactionReceiptView::new(self.clone())
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
    // Genesis and skipped blocks retain transaction bodies without receipts.
    transaction
      .block
      .epoch_block()
      .transaction_receipt_at(transaction.transaction_index)?;

    Some(Self { transaction })
  }

  pub(crate) fn transaction(&self) -> &MinedTransactionView {
    &self.transaction
  }

  pub(crate) fn receipt(&self) -> &Receipt {
    self.receipt_ref().receipt()
  }

  pub(crate) fn execution_error_message(&self) -> Option<&str> {
    self.receipt_ref().execution_error_message()
  }

  fn receipt_ref(&self) -> TransactionReceiptRef<'_> {
    self
      .transaction
      .block
      .epoch_block()
      .transaction_receipt_at(self.transaction.transaction_index)
      .expect("a receipt view must retain its original transaction and receipt")
  }
}
