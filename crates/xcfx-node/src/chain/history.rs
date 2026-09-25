//! Retained chain history, local artifact indexes, and deferred visibility.

use std::{collections::HashMap, sync::Arc};

use cfx_parameters::consensus::DEFERRED_STATE_EPOCH_COUNT;
use cfx_types::H256;
use primitives::block::BlockHeight;

use crate::{execution::ExecutionCommitment, fork::ForkReadError};

use super::{
  BlockIndex, ChainEpoch, CommittedChainView, EpochHistoryView, MinedBlockView,
  MinedTransactionView, TransactionReceiptView,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockLocation {
  epoch_height: BlockHeight,
  block_index: BlockIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TransactionLocation {
  block: BlockLocation,
  transaction_index: usize,
}

pub(crate) struct CommittedChainHistory {
  views: Vec<Arc<CommittedChainView>>,
  block_locations: HashMap<H256, BlockLocation>,
  transaction_locations: HashMap<H256, TransactionLocation>,
}

impl CommittedChainHistory {
  pub(crate) fn from_initial(initial: Arc<CommittedChainView>) -> Self {
    let Some(genesis) = initial.executed_epoch() else {
      return Self {
        views: vec![initial],
        block_locations: HashMap::new(),
        transaction_locations: HashMap::new(),
      };
    };

    let block = genesis.pivot_runtime_block();
    let transactions = block.transactions();
    let block_location = BlockLocation {
      epoch_height: initial.epoch_height(),
      block_index: BlockIndex::Executed(0),
    };
    let block_locations = HashMap::from([(block.hash(), block_location)]);
    let mut transaction_locations = HashMap::with_capacity(transactions.len());

    for (transaction_index, transaction) in transactions.iter().enumerate() {
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
      views: vec![initial],
      block_locations,
      transaction_locations,
    }
  }
  pub(crate) fn capture_checkpoint_head(&self) -> Arc<CommittedChainView> {
    Arc::clone(self.optimistic_head())
  }

  pub(crate) fn views(&self) -> &[Arc<CommittedChainView>] {
    &self.views
  }

  pub(crate) fn contains_view(&self, view: &Arc<CommittedChainView>) -> bool {
    self
      .epoch_at_height(view.epoch_height())
      .is_some_and(|current| Arc::ptr_eq(current, view))
  }

  pub(crate) fn rebuild_through_checkpoint_head(
    &self,
    checkpoint_head: &Arc<CommittedChainView>,
  ) -> Self {
    let checkpoint_height = checkpoint_head.epoch_height();
    let offset = checkpoint_height
      .checked_sub(self.initial_epoch().epoch_height())
      .expect("a checkpoint must not precede the initial view");
    let checkpoint_index =
      usize::try_from(offset).expect("a checkpoint offset must fit in the retained history");
    let ancestor = self
      .views
      .get(checkpoint_index)
      .expect("a checkpoint history head must exist in the current history");

    assert!(
      Arc::ptr_eq(ancestor, checkpoint_head),
      "a checkpoint history head must be an ancestor of the current history",
    );

    let checkpoint_views = &self.views[..=checkpoint_index];
    let (initial, remaining_views) = checkpoint_views
      .split_first()
      .expect("a checkpoint history must contain the initial view");

    let mut history = Self::from_initial(Arc::clone(initial));

    for view in remaining_views {
      history.append_executed_epoch(Arc::clone(view));
    }

    history
  }

  fn initial_epoch(&self) -> &Arc<CommittedChainView> {
    self
      .views
      .first()
      .expect("committed chain history always contains its initial view")
  }

  pub(crate) fn optimistic_head(&self) -> &Arc<CommittedChainView> {
    self
      .views
      .last()
      .expect("committed chain history always contains its initial view")
  }

  pub(crate) fn optimistic_height(&self) -> BlockHeight {
    self.optimistic_head().epoch_height()
  }

  pub(crate) fn append_executed_epoch(&mut self, view: Arc<CommittedChainView>) {
    let epoch = view.local_epoch();
    let epoch_height = view.epoch_height();
    let expected_epoch_height = self
      .optimistic_height()
      .checked_add(1)
      .expect("a committed parent must permit a subsequent epoch height");

    assert_eq!(
      epoch_height, expected_epoch_height,
      "an executed epoch append must contain the next epoch height",
    );
    assert_eq!(
      epoch.ordered_blocks.len(),
      epoch.block_receipts.len(),
      "every executed block must have one block receipt collection",
    );
    assert_eq!(
      epoch.start_block_number,
      self.optimistic_head().next_epoch_start_block_number(),
      "an executed epoch must continue the cumulative Core block number",
    );

    let mut block_locations = HashMap::with_capacity(epoch.ordered_blocks.len());
    let mut transaction_locations = HashMap::new();

    for (block_index, (runtime_block, block_receipts)) in epoch
      .ordered_blocks
      .iter()
      .zip(&epoch.block_receipts)
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

      let block_hash = runtime_block.hash();
      let block_location = BlockLocation {
        epoch_height,
        block_index: BlockIndex::Executed(block_index),
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

    for (index, block) in epoch.skipped_blocks.iter().enumerate() {
      let hash = block.hash();
      assert!(
        !self.block_locations.contains_key(&hash),
        "a skipped block cannot already belong to history"
      );
      assert!(
        block_locations
          .insert(
            hash,
            BlockLocation {
              epoch_height,
              block_index: BlockIndex::Skipped(index),
            }
          )
          .is_none(),
        "a skipped block cannot also execute in the epoch"
      );
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

  pub(crate) fn epoch_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<&Arc<CommittedChainView>> {
    let offset = epoch_height.checked_sub(self.initial_epoch().epoch_height())?;
    let index = usize::try_from(offset).ok()?;
    self.views.get(index)
  }

  pub(crate) fn epoch_history_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<EpochHistoryView> {
    if let ChainEpoch::ForkBase(client) = &self.initial_epoch().epoch {
      if epoch_height <= client.base().epoch_height {
        return Some(EpochHistoryView::Fork {
          client: client.clone(),
          epoch_height,
        });
      }
    }

    // Missing local heights after the base never fall back to the remote chain.
    self
      .epoch_at_height(epoch_height)
      .map(|view| EpochHistoryView::Local(Arc::clone(view)))
  }

  pub(crate) fn latest_state_height(&self) -> BlockHeight {
    self.initial_epoch().epoch_height().max(
      self
        .optimistic_height()
        .saturating_sub(DEFERRED_STATE_EPOCH_COUNT - 1),
    )
  }

  pub(crate) fn latest_state_epoch(&self) -> &Arc<CommittedChainView> {
    self
      .epoch_at_height(self.latest_state_height())
      .expect("a complete linear history must contain its latest state view")
  }

  pub(crate) fn latest_header_committed_height(&self) -> BlockHeight {
    self
      .optimistic_height()
      .saturating_sub(DEFERRED_STATE_EPOCH_COUNT)
  }

  pub(crate) fn latest_header_committed_epoch(&self) -> EpochHistoryView {
    self
      .epoch_history_at_height(self.latest_header_committed_height())
      .expect("a linear history must resolve its latest Header-committed position")
  }

  pub(crate) fn contains_mined_transaction(&self, transaction_hash: &H256) -> bool {
    self.transaction_locations.contains_key(transaction_hash)
  }

  fn mined_block_at(&self, location: BlockLocation) -> MinedBlockView {
    let chain_view = Arc::clone(
      self
        .epoch_at_height(location.epoch_height)
        .expect("an indexed block must reference a committed epoch"),
    );

    MinedBlockView {
      chain_view,
      block_index: location.block_index,
    }
  }

  pub(crate) fn mined_block_by_hash(&self, block_hash: &H256) -> Option<MinedBlockView> {
    self
      .block_locations
      .get(block_hash)
      .copied()
      .map(|location| self.mined_block_at(location))
  }

  fn mined_transaction_at(&self, location: TransactionLocation) -> MinedTransactionView {
    let block = self.mined_block_at(location.block);

    MinedTransactionView {
      block,
      transaction_index: location.transaction_index,
    }
  }

  pub(crate) fn mined_transaction_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<MinedTransactionView> {
    self
      .transaction_locations
      .get(transaction_hash)
      .copied()
      .map(|location| self.mined_transaction_at(location))
  }

  pub(crate) fn transaction_receipt_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<TransactionReceiptView> {
    let location = *self.transaction_locations.get(transaction_hash)?;

    if location.block.epoch_height > self.latest_state_height() {
      return None;
    }

    TransactionReceiptView::new(self.mined_transaction_at(location))
  }

  pub(crate) fn deferred_commitment_for_header_height(
    &self,
    header_height: BlockHeight,
  ) -> Result<ExecutionCommitment, ForkReadError> {
    let deferred_epoch_height = header_height.saturating_sub(DEFERRED_STATE_EPOCH_COUNT);

    self
      .epoch_history_at_height(deferred_epoch_height)
      .expect("a next linear epoch must resolve its deferred execution position")
      .commitment()
  }
}
