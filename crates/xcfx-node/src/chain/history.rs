//! Retained chain history, local artifact indexes, and deferred visibility.

use std::{collections::HashMap, sync::Arc};

use cfx_parameters::consensus::DEFERRED_STATE_EPOCH_COUNT;
use cfx_types::{Address, H256, U256};
use primitives::{BlockNumber, block::BlockHeight};

use crate::{execution::ExecutionCommitment, fork::ForkReadError};

use super::{
  BlockIndex, EpochHistoryView, EpochSource, EpochView, EspaceTransactionView, MinedBlockView,
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

#[derive(Clone, Copy)]
struct EspaceTransactionLocation {
  epoch_height: BlockHeight,
  transaction_index: usize,
}

/// The active epoch sequence and its local block and transaction indexes.
pub(crate) struct ChainHistory {
  views: Vec<Arc<EpochView>>,
  block_locations: HashMap<H256, BlockLocation>,
  transaction_locations: HashMap<H256, TransactionLocation>,
  espace_transaction_locations: HashMap<H256, EspaceTransactionLocation>,
  espace_sender_nonce_locations: HashMap<(Address, U256), EspaceTransactionLocation>,
}

impl ChainHistory {
  pub(crate) fn from_initial(initial: Arc<EpochView>) -> Self {
    let Some(genesis) = initial.artifacts() else {
      return Self {
        views: vec![initial],
        block_locations: HashMap::new(),
        transaction_locations: HashMap::new(),
        espace_transaction_locations: HashMap::new(),
        espace_sender_nonce_locations: HashMap::new(),
      };
    };

    let block = genesis.pivot_runtime_block();
    let transactions = block.transactions();
    let block_location = BlockLocation {
      epoch_height: initial.epoch_height(),
      block_index: BlockIndex::Ordered(0),
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
      espace_transaction_locations: HashMap::new(),
      espace_sender_nonce_locations: HashMap::new(),
    }
  }

  pub(crate) fn views(&self) -> &[Arc<EpochView>] {
    &self.views
  }

  pub(crate) fn contains_view(&self, view: &Arc<EpochView>) -> bool {
    self
      .epoch_at_height(view.epoch_height())
      .is_some_and(|current| Arc::ptr_eq(current, view))
  }

  /// Shares the retained prefix through an ancestor and rebuilds its indexes.
  ///
  /// # Panics
  ///
  /// The ancestor must be the same retained view, not another view at the same height.
  pub(crate) fn rebuild_through(&self, ancestor: &Arc<EpochView>) -> Self {
    let offset = ancestor
      .epoch_height()
      .checked_sub(self.initial_epoch().epoch_height())
      .expect("a history ancestor must not precede the initial view");
    let index =
      usize::try_from(offset).expect("an ancestor offset must fit in the retained history");
    let retained = self
      .views
      .get(index)
      .expect("a history ancestor must exist in the retained history");

    assert!(
      Arc::ptr_eq(retained, ancestor),
      "a history ancestor must be the same retained epoch view",
    );

    let (initial, remaining_views) = self.views[..=index]
      .split_first()
      .expect("a history prefix must contain the initial view");

    let mut history = Self::from_initial(Arc::clone(initial));

    for view in remaining_views {
      history.append_executed_epoch(Arc::clone(view));
    }

    history
  }

  fn initial_epoch(&self) -> &Arc<EpochView> {
    self
      .views
      .first()
      .expect("committed chain history always contains its initial view")
  }

  pub(crate) fn optimistic_head(&self) -> &Arc<EpochView> {
    self
      .views
      .last()
      .expect("committed chain history always contains its initial view")
  }

  pub(crate) fn optimistic_height(&self) -> BlockHeight {
    self.optimistic_head().epoch_height()
  }

  pub(crate) fn append_executed_epoch(&mut self, view: Arc<EpochView>) {
    let artifacts = view.local_artifacts();
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
      artifacts.start_block_number,
      self.optimistic_head().next_epoch_start_block_number(),
      "an executed epoch must continue the cumulative Core block number",
    );

    let mut block_locations = HashMap::with_capacity(artifacts.ordered_blocks.len());
    let mut transaction_locations = HashMap::new();

    for (block_index, block) in artifacts.ordered_blocks.iter().enumerate() {
      let block_hash = block.runtime_block().hash();
      let block_location = BlockLocation {
        epoch_height,
        block_index: BlockIndex::Ordered(block_index),
      };

      assert!(
        !self.block_locations.contains_key(&block_hash),
        "a committed block hash must not already exist in history",
      );
      assert!(
        block_locations.insert(block_hash, block_location).is_none(),
        "a committed epoch must not contain duplicate block hashes",
      );

      for result in block.transaction_receipts() {
        if result.receipt().tx_skipped() {
          continue;
        }

        let transaction_hash = result.transaction().hash();
        let transaction_location = TransactionLocation {
          block: block_location,
          transaction_index: result.block_transaction_index(),
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

    for (index, block) in artifacts.skipped_blocks.iter().enumerate() {
      let hash = block.runtime_block().hash();
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

    let mut espace_transaction_locations = HashMap::new();
    let mut espace_sender_nonce_locations = HashMap::new();
    for (transaction_index, transaction) in artifacts.espace_transactions().enumerate() {
      let hash = transaction.hash();
      let location = EspaceTransactionLocation {
        epoch_height,
        transaction_index,
      };
      assert!(
        !self.espace_transaction_locations.contains_key(&hash),
        "an eSpace transaction hash must not already exist in history",
      );
      assert!(
        espace_transaction_locations
          .insert(hash, location,)
          .is_none(),
        "an epoch must not contain duplicate eSpace transaction hashes",
      );
      // Phantom entries may repeat sender/nonce. Projection order selects the last match.
      espace_sender_nonce_locations.insert(
        (transaction.sender(), *transaction.transaction().nonce()),
        location,
      );
    }

    self.views.reserve(1);
    self.block_locations.reserve(block_locations.len());
    self
      .transaction_locations
      .reserve(transaction_locations.len());
    self
      .espace_transaction_locations
      .reserve(espace_transaction_locations.len());
    self
      .espace_sender_nonce_locations
      .reserve(espace_sender_nonce_locations.len());

    self.views.push(view);
    self.block_locations.extend(block_locations);
    self.transaction_locations.extend(transaction_locations);
    self
      .espace_transaction_locations
      .extend(espace_transaction_locations);
    self
      .espace_sender_nonce_locations
      .extend(espace_sender_nonce_locations);
  }

  pub(crate) fn epoch_at_height(&self, epoch_height: BlockHeight) -> Option<&Arc<EpochView>> {
    let offset = epoch_height.checked_sub(self.initial_epoch().epoch_height())?;
    let index = usize::try_from(offset).ok()?;
    self.views.get(index)
  }

  pub(crate) fn epoch_history_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<EpochHistoryView> {
    if let EpochSource::ForkBase(client) = &self.initial_epoch().source {
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

  pub(crate) fn latest_state_epoch(&self) -> &Arc<EpochView> {
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

  /// Original transactions only: phantom query identities must not affect
  /// admission, duplicate execution checks, or reorg pool reconciliation.
  pub(crate) fn contains_mined_transaction(&self, transaction_hash: &H256) -> bool {
    self.transaction_locations.contains_key(transaction_hash)
  }

  fn mined_block_at(&self, location: BlockLocation) -> MinedBlockView {
    let epoch_view = Arc::clone(
      self
        .epoch_at_height(location.epoch_height)
        .expect("an indexed block must reference a committed epoch"),
    );

    MinedBlockView {
      epoch_view,
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

  /// Pins a local block by its Core execution-order number.
  /// Skipped blocks have no number; the fixed Fork Base has no local artifacts.
  pub(crate) fn mined_block_by_number(&self, number: BlockNumber) -> Option<MinedBlockView> {
    let epoch_index = self
      .views
      .partition_point(|view| view.pivot_block_number() < number);
    let epoch_view = self.views.get(epoch_index)?;
    let artifacts = epoch_view.artifacts()?;

    // Appends establish contiguous ranges, so the first pivot at or above
    // this number contains it. The offset is bounded by the epoch's block count.
    let block_index = usize::try_from(number - artifacts.start_block_number)
      .expect("a Core block number within its epoch must fit the block index");

    Some(MinedBlockView {
      epoch_view: Arc::clone(epoch_view),
      block_index: BlockIndex::Ordered(block_index),
    })
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

  fn mined_espace_transaction_at(
    &self,
    location: EspaceTransactionLocation,
  ) -> EspaceTransactionView {
    self
      .epoch_at_height(location.epoch_height)
      .expect("an indexed eSpace transaction must reference a retained epoch")
      .espace_block()
      .expect("an indexed eSpace transaction must have local artifacts")
      .transaction_at(location.transaction_index)
      .expect("an eSpace index must reference a transaction in the retained projection")
  }

  pub(crate) fn mined_espace_transaction_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<EspaceTransactionView> {
    self
      .espace_transaction_locations
      .get(transaction_hash)
      .copied()
      .map(|location| self.mined_espace_transaction_at(location))
  }

  pub(crate) fn mined_espace_transaction_by_sender_and_nonce(
    &self,
    sender: Address,
    nonce: U256,
  ) -> Option<EspaceTransactionView> {
    self
      .espace_sender_nonce_locations
      .get(&(sender, nonce))
      .copied()
      .map(|location| self.mined_espace_transaction_at(location))
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
