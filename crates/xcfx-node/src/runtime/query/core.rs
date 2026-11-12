//! Core queries over a selected committed state or fixed remote history.

use std::sync::Arc;

use cfx_types::{Address, H256, Space, U256};
use primitives::{BlockNumber, block::BlockHeight};
use thiserror::Error;

use crate::{
  block_producer::RuntimeBlock,
  chain::{EpochHistoryView, MinedBlockView, MinedTransactionView, TransactionReceiptView},
  fork::{ForkClient, ForkReadError, HistoryBlockId},
  native_state,
  runtime_transaction::RuntimeTransaction,
};

use super::{
  EpochSelector, NodeRuntime, StateQueryError, StateSelector, StateSource, StateUnavailable,
};

#[derive(Debug, Error)]
#[error("epoch number exceeds the current chain head")]
pub(crate) struct EpochNumberTooLarge;

#[derive(Debug, Error)]
pub(crate) enum CoreEpochQueryError {
  #[error(transparent)]
  Epoch(#[from] EpochNumberTooLarge),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
}

#[derive(Debug, Error)]
pub(crate) enum CoreBlockSelectionError {
  #[error(transparent)]
  Epoch(#[from] EpochNumberTooLarge),
  #[error("pivot chain assumption failed")]
  PivotAssumption,
}

#[derive(Debug, Error)]
pub(crate) enum CoreStorageRootError {
  #[error(transparent)]
  Unavailable(#[from] StateUnavailable),
  #[error("storage roots are not available for a forked state")]
  Unsupported,
}

#[derive(Clone, Copy)]
pub(crate) enum CoreBlockSelector {
  Hash(H256),
  Epoch(EpochSelector),
  Number(BlockNumber),
}

pub(crate) enum CoreBlockSource<'a> {
  Local(MinedBlockView),
  Detached(Arc<RuntimeBlock>),
  Fork {
    client: &'a ForkClient,
    selector: HistoryBlockId,
  },
}

/// A local block satisfying the assumption, or one fixed-prefix remote read.
pub(crate) enum CoreBlockWithPivotSource<'a> {
  Local(MinedBlockView),
  Fork {
    client: &'a ForkClient,
    hash: H256,
    pivot: H256,
    epoch_height: BlockHeight,
  },
}

pub(crate) enum CoreTransactionSource<'a> {
  Pool(&'a RuntimeTransaction),
  Local(MinedTransactionView),
  Fork { client: &'a ForkClient, hash: H256 },
}

pub(crate) enum CoreReceiptSource<'a> {
  Local(TransactionReceiptView),
  Fork { client: &'a ForkClient, hash: H256 },
}

pub(crate) enum CoreEpochReceiptsSource<'a> {
  Local(Vec<MinedBlockView>),
  Fork {
    client: &'a ForkClient,
    epoch_height: BlockHeight,
  },
}

pub(crate) enum CoreDebugTransactionsSource<'a> {
  Local(Vec<MinedBlockView>),
  ForkEpoch {
    client: &'a ForkClient,
    epoch_height: BlockHeight,
  },
  ForkBlock {
    client: &'a ForkClient,
    hash: H256,
  },
}

impl NodeRuntime {
  pub(crate) fn core_admin_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<Option<Address>, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().admin(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_admin_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_sponsor_info_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<native_state::SponsorInfo, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .sponsor_info(address)?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_sponsor_info_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_staking_balance_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .staking_balance(address)?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_staking_balance_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_deposit_list_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<Vec<primitives::DepositInfo>, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .deposit_list(address)?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_deposit_list_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_vote_list_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<Vec<primitives::VoteStakeInfo>, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().vote_list(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_vote_list_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_collateral_for_storage_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .collateral_for_storage(address)?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_collateral_for_storage_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_account_at(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<native_state::AccountInfo, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().account(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_account_at(address, epoch_height)?),
    }
  }

  pub(crate) fn core_interest_rate_at(
    &self,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().interest_rate()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_interest_rate_at(epoch_height)?),
    }
  }

  pub(crate) fn core_accumulated_interest_rate_at(
    &self,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .accumulated_interest_rate()?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_accumulated_interest_rate_at(epoch_height)?),
    }
  }

  pub(crate) fn core_pos_economics_at(
    &self,
    selector: EpochSelector,
  ) -> Result<native_state::PosEconomics, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().pos_economics()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_pos_economics_at(epoch_height)?),
    }
  }

  pub(crate) fn core_supply_info_at(
    &self,
    selector: EpochSelector,
  ) -> Result<native_state::TokenSupplyInfo, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().supply_info()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_supply_info_at(epoch_height)?),
    }
  }

  pub(crate) fn core_collateral_info_at(
    &self,
    selector: EpochSelector,
  ) -> Result<native_state::StorageCollateralInfo, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().collateral_info()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_collateral_info_at(epoch_height)?),
    }
  }

  pub(crate) fn core_fee_burnt_at(&self, selector: EpochSelector) -> Result<U256, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().fee_burnt()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_fee_burnt_at(epoch_height)?),
    }
  }

  pub(crate) fn core_vote_params_at(
    &self,
    selector: EpochSelector,
  ) -> Result<native_state::VoteParams, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_reader().vote_params()?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_vote_params_at(epoch_height)?),
    }
  }

  pub(crate) fn core_check_balance_at(
    &self,
    request: native_state::BalanceCheckRequest,
    selector: EpochSelector,
  ) -> Result<native_state::BalanceCheckResult, StateQueryError> {
    match self.state_source(StateSelector::Epoch(selector))? {
      StateSource::Local(epoch) => Ok(
        epoch
          .state()
          .version
          .native_reader()
          .check_balance(request)?,
      ),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_check_balance_at(request, epoch_height)?),
    }
  }

  pub(crate) fn core_debug_epoch_source(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<CoreDebugTransactionsSource<'_>, EpochNumberTooLarge> {
    self
      .core_epoch_number(EpochSelector::Number(epoch_height))
      .ok_or(EpochNumberTooLarge)?;

    if let Some(client) = self.reset_state.initial_view.fork_client()
      && epoch_height <= client.base().epoch_height
    {
      return Ok(CoreDebugTransactionsSource::ForkEpoch {
        client,
        epoch_height,
      });
    }

    let history = &self.runtime_state.history;
    let epoch = history
      .epoch_at_height(epoch_height)
      .expect("a selected local debug epoch must be retained");

    let blocks = epoch
      .artifacts()
      .expect("a local epoch has artifacts")
      .ordered_blocks()
      .iter()
      .map(|block| {
        history
          .mined_block_by_hash(&block.runtime_block().hash())
          .expect("an ordered block must be indexed")
      })
      .collect();

    Ok(CoreDebugTransactionsSource::Local(blocks))
  }

  pub(crate) fn core_debug_block_source(
    &self,
    hash: H256,
  ) -> Option<CoreDebugTransactionsSource<'_>> {
    if let Some(block) = self.runtime_state.history.mined_block_by_hash(&hash) {
      let blocks = if block.block_number().is_some() {
        vec![block]
      } else {
        Vec::new()
      };

      return Some(CoreDebugTransactionsSource::Local(blocks));
    }

    if self.runtime_state.graph.block(&hash).is_some() {
      return None;
    }

    self
      .reset_state
      .initial_view
      .fork_client()
      .map(|client| CoreDebugTransactionsSource::ForkBlock { client, hash })
  }

  pub(crate) fn core_storage_root(
    &self,
    address: Address,
    selector: EpochSelector,
  ) -> Result<primitives::StorageRoot, CoreStorageRootError> {
    match self.state_epoch_source(selector)? {
      StateSource::Local(epoch) => epoch
        .state()
        .version
        .native_storage_root(address)
        .ok_or(CoreStorageRootError::Unsupported),

      StateSource::Fork { .. } => Err(CoreStorageRootError::Unsupported),
    }
  }

  pub(crate) fn core_confirmation_risk(&self, hash: H256) -> Result<Option<U256>, ForkReadError> {
    let initial = &self.reset_state.initial_view;
    let height = if hash == initial.state().epoch_id {
      Some(initial.epoch_height())
    } else if let Some(block) = self.runtime_state.history.mined_block_by_hash(&hash) {
      Some(block.epoch_height())
    } else if self.runtime_state.graph.block(&hash).is_some() {
      None
    } else if let Some(client) = initial.fork_client() {
      client.history_epoch_by_hash(hash, false)?
    } else {
      None
    };
    Ok(height.map(|height| {
      if height == 0 {
        U256::zero()
      } else if height <= self.epoch_positions().confirmed.height {
        U256::MAX / U256::from(100_000_000u64)
      } else {
        U256::MAX / U256::from(10_000u64)
      }
    }))
  }

  /// Resolves Core block identity without reading remote data or inferring an epoch for detached blocks.
  pub(crate) fn core_block_source(
    &self,
    selector: CoreBlockSelector,
  ) -> Result<Option<CoreBlockSource<'_>>, EpochNumberTooLarge> {
    let history = &self.runtime_state.history;
    let client = self.reset_state.initial_view.fork_client();
    Ok(match selector {
      CoreBlockSelector::Hash(hash) => {
        if let Some(block) = history.mined_block_by_hash(&hash) {
          Some(CoreBlockSource::Local(block))
        } else if let Some(block) = self.runtime_state.graph.block(&hash) {
          Some(CoreBlockSource::Detached(Arc::clone(block)))
        } else {
          client.map(|client| CoreBlockSource::Fork {
            client,
            selector: HistoryBlockId::Hash(hash),
          })
        }
      }
      CoreBlockSelector::Epoch(epoch) => {
        let height = self.core_epoch_number(epoch).ok_or(EpochNumberTooLarge)?;
        if let Some(client) = client
          && height <= client.base().epoch_height
        {
          Some(CoreBlockSource::Fork {
            client,
            selector: HistoryBlockId::Epoch(height),
          })
        } else {
          let epoch = history
            .epoch_at_height(height)
            .expect("a resolved local block epoch must be retained");
          Some(CoreBlockSource::Local(
            history
              .mined_block_by_hash(&epoch.state().epoch_id)
              .expect("a local pivot must be indexed"),
          ))
        }
      }
      CoreBlockSelector::Number(number) => {
        if let Some(client) = client
          && number <= client.base().pivot_block_number
        {
          Some(CoreBlockSource::Fork {
            client,
            selector: HistoryBlockId::Number(number),
          })
        } else {
          history
            .mined_block_by_number(number)
            .map(CoreBlockSource::Local)
        }
      }
    })
  }

  pub(crate) fn core_block_with_pivot_source(
    &self,
    hash: H256,
    pivot: H256,
    epoch_height: BlockHeight,
  ) -> Result<CoreBlockWithPivotSource<'_>, CoreBlockSelectionError> {
    self
      .core_epoch_number(EpochSelector::Number(epoch_height))
      .ok_or(EpochNumberTooLarge)?;
    let history = &self.runtime_state.history;

    if let Some(client) = self.reset_state.initial_view.fork_client()
      && epoch_height <= client.base().epoch_height
    {
      if history.mined_block_by_hash(&hash).is_some()
        || self.runtime_state.graph.block(&hash).is_some()
      {
        return Err(CoreBlockSelectionError::PivotAssumption);
      }
      return Ok(CoreBlockWithPivotSource::Fork {
        client,
        hash,
        pivot,
        epoch_height,
      });
    }

    let epoch = history
      .epoch_at_height(epoch_height)
      .expect("a selected local block epoch must be retained");
    if epoch.state().epoch_id != pivot {
      return Err(CoreBlockSelectionError::PivotAssumption);
    }
    let block = history
      .mined_block_by_hash(&hash)
      .filter(|block| block.epoch_height() == epoch_height)
      .ok_or(CoreBlockSelectionError::PivotAssumption)?;
    Ok(CoreBlockWithPivotSource::Local(block))
  }

  pub(crate) fn core_transaction_source(&self, hash: H256) -> Option<CoreTransactionSource<'_>> {
    if let Some(transaction) = self.runtime_state.history.mined_transaction_by_hash(&hash) {
      return (transaction.transaction().space() == Space::Native)
        .then_some(CoreTransactionSource::Local(transaction));
    }
    if let Some(transaction) = self.pool_transaction_by_hash(hash) {
      return (transaction.space() == Space::Native)
        .then_some(CoreTransactionSource::Pool(transaction));
    }
    if self
      .runtime_state
      .history
      .mined_espace_transaction_by_hash(&hash)
      .is_some()
    {
      return None;
    }
    self
      .reset_state
      .initial_view
      .fork_client()
      .map(|client| CoreTransactionSource::Fork { client, hash })
  }

  pub(crate) fn core_receipt_source(&self, hash: H256) -> Option<CoreReceiptSource<'_>> {
    match self.core_transaction_source(hash)? {
      CoreTransactionSource::Pool(_) => None,
      CoreTransactionSource::Local(transaction) => {
        if transaction.block().epoch_height() > self.runtime_state.history.latest_state_height() {
          return None;
        }
        transaction.receipt().map(CoreReceiptSource::Local)
      }
      CoreTransactionSource::Fork { client, hash } => {
        Some(CoreReceiptSource::Fork { client, hash })
      }
    }
  }

  pub(crate) fn core_epoch_receipts_source(
    &self,
    selector: StateSelector,
  ) -> Result<Option<CoreEpochReceiptsSource<'_>>, super::StateSelectionError> {
    let epoch = self.state_epoch_selector(selector)?;
    let height = self.epoch_height(epoch);
    if height > self.runtime_state.history.optimistic_height() {
      return Err(
        super::StateUnavailable {
          epoch_height: height,
        }
        .into(),
      );
    }
    if height > self.runtime_state.history.latest_state_height() {
      return Ok(None);
    }
    if let Some(client) = self.reset_state.initial_view.fork_client()
      && height <= client.base().epoch_height
    {
      return Ok(Some(CoreEpochReceiptsSource::Fork {
        client,
        epoch_height: height,
      }));
    }
    let epoch = self
      .runtime_state
      .history
      .epoch_at_height(height)
      .expect("a local receipt epoch must be retained");
    let blocks = epoch
      .artifacts()
      .expect("a local epoch has artifacts")
      .ordered_blocks()
      .iter()
      .map(|block| {
        self
          .runtime_state
          .history
          .mined_block_by_hash(&block.runtime_block().hash())
          .expect("an ordered local block must be indexed")
      })
      .collect();
    Ok(Some(CoreEpochReceiptsSource::Local(blocks)))
  }

  /// Resolves a Core epoch query, rejecting an explicit number beyond the chain head.
  /// This does not require an executable state at the resulting height.
  pub(crate) fn core_epoch_number(&self, selector: EpochSelector) -> Option<BlockHeight> {
    match selector {
      EpochSelector::Number(height) if height > self.runtime_state.history.optimistic_height() => {
        None
      }
      selector => Some(self.epoch_height(selector)),
    }
  }

  /// Returns blocks in epoch execution order, with the pivot last.
  ///
  /// # Errors
  /// Rejects an epoch beyond the mined tip and propagates fixed-prefix read failures.
  pub(crate) fn core_blocks_by_epoch(
    &self,
    selector: EpochSelector,
  ) -> Result<Arc<[H256]>, CoreEpochQueryError> {
    match self.core_history_epoch(selector)? {
      EpochHistoryView::Local(epoch) => Ok(
        epoch
          .artifacts()
          .expect("a local history epoch has execution artifacts")
          .ordered_blocks()
          .iter()
          .map(|block| block.runtime_block().hash())
          .collect::<Vec<_>>()
          .into(),
      ),
      EpochHistoryView::Fork {
        client,
        epoch_height,
      } => Ok(client.block_hashes(epoch_height)?),
    }
  }

  /// Returns the epoch's skipped prefix in its retained order.
  ///
  /// # Errors
  /// Rejects an epoch beyond the mined tip and propagates fixed-prefix read failures.
  pub(crate) fn core_skipped_blocks_by_epoch(
    &self,
    selector: EpochSelector,
  ) -> Result<Arc<[H256]>, CoreEpochQueryError> {
    match self.core_history_epoch(selector)? {
      EpochHistoryView::Local(epoch) => Ok(
        epoch
          .artifacts()
          .expect("a local history epoch has execution artifacts")
          .skipped_blocks()
          .iter()
          .map(|block| block.runtime_block().hash())
          .collect::<Vec<_>>()
          .into(),
      ),
      EpochHistoryView::Fork {
        client,
        epoch_height,
      } => Ok(client.skipped_block_hashes(epoch_height)?),
    }
  }

  fn core_history_epoch(
    &self,
    selector: EpochSelector,
  ) -> Result<EpochHistoryView, EpochNumberTooLarge> {
    let height = self
      .core_epoch_number(selector)
      .ok_or(EpochNumberTooLarge)?;
    Ok(
      self
        .epoch_history_at_height(height)
        .expect("an epoch within the mined range has a local or fixed-prefix source"),
    )
  }

  /// Reads the selected balance without reconstructing a remote execution account.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_balance_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_balance(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_balance_at(address, epoch_height)?),
    }
  }

  /// Reads only the selected committed nonce, independently of pool occupancy.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_nonce_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<U256, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_nonce(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_nonce_at(address, epoch_height)?),
    }
  }

  /// Returns code bytes without supplying missing remote collateral-owner metadata.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_code_at(
    &self,
    address: Address,
    selector: StateSelector,
  ) -> Result<Arc<Vec<u8>>, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_code(address)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_code_at(address, epoch_height)?),
    }
  }

  /// Preserves the distinction between an absent storage entry and its stored value.
  ///
  /// # Errors
  /// Rejects unavailable positions and propagates state or fixed-prefix read failures.
  pub(crate) fn core_storage_at(
    &self,
    address: Address,
    position: H256,
    selector: StateSelector,
  ) -> Result<Option<H256>, StateQueryError> {
    match self.state_source(selector)? {
      StateSource::Local(epoch) => Ok(epoch.state().version.native_storage(address, position)?),
      StateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.core_storage_at(address, position, epoch_height)?),
    }
  }
}
