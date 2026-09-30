//! Read-only access to retained history, state versions, and instance metadata.

use std::sync::Arc;

use cfx_statedb::Result as StateResult;
use cfx_types::{Address, AddressWithSpace, H256, Space, U256};
use primitives::block::BlockHeight;
use thiserror::Error;

use crate::{
  chain::{
    EpochHistoryView, EpochView, MinedBlockView, MinedTransactionView, TransactionReceiptView,
  },
  chain_spec::ChainSpec,
  fork::ForkBase,
  state::state_version::StateVersion,
};

use super::NodeRuntime;

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

impl NodeRuntime {
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
      EpochSelector::Confirmed => self.runtime_state.stability.confirmed.height,
      EpochSelector::Finalized => self.runtime_state.stability.finalized.height,
      EpochSelector::Checkpoint => self.runtime_state.stability.checkpoint.height,
    }
  }

  /// Pins a committed state version selected under the current visibility rules.
  ///
  /// Later chain changes do not modify this version. Uncached Fork reads
  /// still require the instance's remote read service.
  ///
  /// # Errors
  ///
  /// Returns an error if the selected epoch precedes the initial view
  /// or exceeds `latest_state`.
  pub(crate) fn state_at(
    &self,
    selector: EpochSelector,
  ) -> Result<Arc<StateVersion>, StateUnavailable> {
    let epoch = self.epoch_for_state(selector)?;
    Ok(Arc::clone(&epoch.state().version))
  }

  pub(super) fn epoch_for_state(
    &self,
    selector: EpochSelector,
  ) -> Result<&EpochView, StateUnavailable> {
    let epoch_height = self.epoch_height(selector);
    let earliest = self.reset_state.initial_view.epoch_height();
    let latest = self.runtime_state.history.latest_state_height();

    if epoch_height < earliest || epoch_height > latest {
      return Err(StateUnavailable { epoch_height });
    }

    let epoch = self
      .runtime_state
      .history
      .epoch_at_height(epoch_height)
      .expect("retained history must contain every epoch in the queryable state range");

    Ok(epoch.as_ref())
  }

  /// Finds the next pool nonce from the effective state.
  ///
  /// Pool presence does not imply balance or sponsorship readiness.
  /// The returned value need not satisfy the transaction admission nonce limit.
  /// Returns `Ok(None)` only if advancing the contiguous pool sequence overflows `U256`.
  ///
  /// # Errors
  ///
  /// Propagates errors from reading the effective state.
  pub(crate) fn pending_nonce(&self, address: AddressWithSpace) -> StateResult<Option<U256>> {
    let state_nonce = self.open_effective_state_for_reading()?.nonce(&address)?;

    Ok(
      self
        .runtime_state
        .transaction_pool
        .next_nonce(address, state_nonce),
    )
  }
}
