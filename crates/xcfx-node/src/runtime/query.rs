//! Read-only access to retained history, state versions, and instance metadata.

use std::sync::Arc;

use cfx_executor::state::State;
use cfx_types::{Address, AddressWithSpace, H256, Space, U256};
use primitives::{BlockNumber, block::BlockHeight};
use thiserror::Error;

use crate::{
  chain::{
    EpochHistoryView, EpochView, EspaceTransactionView, MinedBlockView, MinedTransactionView,
    TransactionReceiptView,
  },
  chain_spec::ChainSpec,
  fork::{ForkBase, ForkClient},
  state::state_version::StateVersion,
};

use super::{EpochPositions, NodeRuntime};

#[derive(Clone, Copy, Debug)]
pub(crate) enum EpochSelector {
  Number(BlockHeight),
  Earliest,
  LatestMined,
  LatestState,
  Confirmed,
  Finalized,
  Checkpoint,
}

#[derive(Debug, Error)]
#[error("state at epoch {epoch_height} is unavailable")]
pub(crate) struct StateUnavailable {
  pub(crate) epoch_height: BlockHeight,
}

#[derive(Debug, Error)]
pub(crate) enum StateQueryError {
  #[error(transparent)]
  Unavailable(#[from] StateUnavailable),
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),
}

/// A receipt source selected under the current pool and history. A remote
/// lookup borrows the Runtime and retains the hash to which that decision applies.
pub(crate) enum EspaceReceiptSource<'a> {
  Local(EspaceTransactionView),
  Fork { client: &'a ForkClient, hash: H256 },
}

impl NodeRuntime {
  pub(crate) fn espace_chain_id(&self) -> u32 {
    self
      .chain_spec
      .machine()
      .params()
      .chain_id
      .read()
      .get_chain_id(self.runtime_state.history.optimistic_height())
      .in_evm_space()
  }

  pub(crate) fn optimistic_head(&self) -> Arc<EpochView> {
    Arc::clone(self.runtime_state.history.optimistic_head())
  }

  /// The original remote identity, unchanged by local execution or reset.
  pub(crate) fn fork_base(&self) -> Option<&ForkBase> {
    self.reset_state.initial_view.fork_base()
  }

  /// A retained state view, starting at Local Genesis or the fixed Fork Base.
  /// Earlier remote epochs are available only through `epoch_history_at_height`.
  /// This may return epochs beyond `latest_state`; use `state_at` for visible state.
  pub(crate) fn epoch_at_height(&self, epoch_height: BlockHeight) -> Option<Arc<EpochView>> {
    self
      .runtime_state
      .history
      .epoch_at_height(epoch_height)
      .map(Arc::clone)
  }

  /// Resolves history by execution height. Positions through the fixed Fork
  /// Base use remote history; later positions must exist in local history.
  pub(crate) fn epoch_history_at_height(
    &self,
    epoch_height: BlockHeight,
  ) -> Option<EpochHistoryView> {
    self
      .runtime_state
      .history
      .epoch_history_at_height(epoch_height)
  }

  /// Looks up local mined artifacts. Remote history requires an epoch-bound view.
  pub(crate) fn mined_block_by_hash(&self, block_hash: &H256) -> Option<MinedBlockView> {
    self.runtime_state.history.mined_block_by_hash(block_hash)
  }

  /// Pins a locally retained block by Core execution-order number.
  /// Fork Base and earlier blocks require remote history; skipped blocks have no number.
  pub(crate) fn mined_block_by_number(&self, number: BlockNumber) -> Option<MinedBlockView> {
    self.runtime_state.history.mined_block_by_number(number)
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

  /// Local receipt visibility. `None` also covers Genesis and not-yet-visible
  /// transactions; it does not by itself permit a remote history fallback.
  pub(crate) fn transaction_receipt_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<TransactionReceiptView> {
    self
      .runtime_state
      .history
      .transaction_receipt_by_hash(transaction_hash)
  }

  /// Includes ordinary eSpace transactions and phantom entries in retained local
  /// epochs, even beyond `latest_state`. Does not read the pool or remote history.
  pub(crate) fn mined_espace_transaction_by_hash(
    &self,
    transaction_hash: &H256,
  ) -> Option<EspaceTransactionView> {
    self
      .runtime_state
      .history
      .mined_espace_transaction_by_hash(transaction_hash)
  }

  /// Selects a complete eSpace receipt lookup without performing remote IO.
  ///
  /// Pool entries and local transactions without a visible eSpace receipt end
  /// the lookup here. Only absence from both local indexes permits a Fork read.
  pub(crate) fn espace_receipt_source(
    &self,
    transaction_hash: &H256,
  ) -> Option<EspaceReceiptSource<'_>> {
    if self
      .runtime_state
      .transaction_pool
      .get_by_hash(*transaction_hash)
      .is_some()
    {
      return None;
    }

    let history = &self.runtime_state.history;
    if let Some(transaction) = history.mined_espace_transaction_by_hash(transaction_hash) {
      return (transaction.block().epoch_height() <= history.latest_state_height())
        .then_some(EspaceReceiptSource::Local(transaction));
    }
    if history.contains_mined_transaction(transaction_hash) {
      return None;
    }

    self
      .reset_state
      .initial_view
      .fork_client()
      .map(|client| EspaceReceiptSource::Fork {
        client,
        hash: *transaction_hash,
      })
  }

  pub(crate) fn latest_state_epoch(&self) -> Arc<EpochView> {
    Arc::clone(self.runtime_state.history.latest_state_epoch())
  }

  pub(crate) fn latest_header_committed_epoch(&self) -> EpochHistoryView {
    self.runtime_state.history.latest_header_committed_epoch()
  }

  /// Retained views in ascending epoch order, starting at Genesis or the Fork Base.
  ///
  /// Includes epochs beyond `latest_state`; use `state_at` for visible state.
  pub(crate) fn local_views(&self) -> &[Arc<EpochView>] {
    self.runtime_state.history.views()
  }

  pub(crate) fn chain_spec(&self) -> &ChainSpec {
    self.chain_spec.as_ref()
  }

  /// Lists key-backed accounts; impersonation does not add entries.
  pub(crate) fn signing_accounts(&self, space: Space) -> Vec<Address> {
    self.signing_keys.addresses(space)
  }

  /// Resolves a position without checking whether its history or state is available.
  pub(crate) fn epoch_height(&self, selector: EpochSelector) -> BlockHeight {
    match selector {
      EpochSelector::Number(height) => height,
      EpochSelector::Earliest => 0,
      EpochSelector::LatestMined => self.runtime_state.history.optimistic_height(),
      EpochSelector::LatestState => self.runtime_state.history.latest_state_height(),
      EpochSelector::Confirmed => self.runtime_state.epoch_positions.confirmed.height,
      EpochSelector::Finalized => self.runtime_state.epoch_positions.finalized.height,
      EpochSelector::Checkpoint => self.runtime_state.epoch_positions.checkpoint.height,
    }
  }

  pub(crate) fn epoch_positions(&self) -> EpochPositions {
    self.runtime_state.epoch_positions
  }

  /// Pins a committed state version selected under the current visibility rules.
  ///
  /// Stable tags resolve retained executed states even when a rollback moves
  /// `latest_state` below them. Other selectors use the deferred state boundary.
  ///
  /// Later chain changes do not modify this version. Uncached Fork reads
  /// still require the instance's remote read service.
  ///
  /// # Errors
  ///
  /// Returns an error if the selected epoch precedes the initial view or exceeds
  /// the selector's readable range.
  pub(crate) fn state_at(
    &self,
    selector: EpochSelector,
  ) -> Result<Arc<StateVersion>, StateUnavailable> {
    let epoch = self.epoch_for_state(selector)?;
    Ok(Arc::clone(&epoch.state().version))
  }

  /// Reads an account balance from the selected committed state.
  ///
  /// Uncached Fork reads may block and require the instance's read service.
  ///
  /// # Errors
  ///
  /// Returns an error if the selected state is unavailable or reading it fails.
  pub(crate) fn balance_at(
    &self,
    address: AddressWithSpace,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    let version = self.state_at(selector)?;
    let state = State::new(version.open_database())?;
    Ok(state.balance(&address)?)
  }

  pub(super) fn epoch_for_state(
    &self,
    selector: EpochSelector,
  ) -> Result<&EpochView, StateUnavailable> {
    let epoch_height = self.epoch_height(selector);
    let earliest = self.reset_state.initial_view.epoch_height();
    let latest_readable = match selector {
      EpochSelector::Confirmed | EpochSelector::Finalized | EpochSelector::Checkpoint => {
        self.runtime_state.history.optimistic_height()
      }
      EpochSelector::Number(_)
      | EpochSelector::Earliest
      | EpochSelector::LatestMined
      | EpochSelector::LatestState => self.runtime_state.history.latest_state_height(),
    };

    if epoch_height < earliest || epoch_height > latest_readable {
      return Err(StateUnavailable { epoch_height });
    }

    let epoch = self
      .runtime_state
      .history
      .epoch_at_height(epoch_height)
      .expect("retained history must contain every epoch in the queryable state range");

    Ok(epoch.as_ref())
  }
}
