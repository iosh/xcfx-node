//! eSpace query positions and pool/local/fixed-prefix source selection.

use std::sync::Arc;

use cfx_executor::state::State;
use cfx_types::{Address, AddressSpaceUtil, BigEndianHash, H256, Space, U256};
use primitives::block::BlockHeight;
use thiserror::Error;

use crate::{
  chain::{EpochView, EspaceBlockView, EspaceTransactionView},
  fork::{ForkClient, ForkReadError, HistoryBlockId},
  runtime_transaction::RuntimeTransaction,
};

use super::{EpochSelector, NodeRuntime, StateUnavailable};

#[derive(Clone, Copy)]
pub(crate) enum EspaceBlockSelector {
  Epoch(EpochSelector),
  Hash(H256),
}

#[derive(Clone, Copy)]
pub(crate) enum EspaceStateSelector {
  Epoch(EpochSelector),
  Hash { hash: H256, require_pivot: bool },
}

#[derive(Debug, Error)]
pub(crate) enum EspaceStateQueryError {
  #[error(transparent)]
  Unavailable(#[from] StateUnavailable),
  #[error("unknown or inapplicable state block {0:?}")]
  UnknownBlock(H256),
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
}

/// Remote sources borrow the Runtime until NodeCall completes the read.
pub(crate) enum EspaceBlockSource<'a> {
  Local(EspaceBlockView),
  Fork {
    client: &'a ForkClient,
    selector: HistoryBlockId,
  },
}

/// Local identity lookup never requires a remote read.
pub(crate) enum LocalEspaceTransactionSource<'a> {
  Pool(&'a RuntimeTransaction),
  Mined(EspaceTransactionView),
}

pub(crate) enum EspaceTransactionSource<'a> {
  Local(LocalEspaceTransactionSource<'a>),
  Fork { client: &'a ForkClient, hash: H256 },
}

enum EspaceStateSource<'a> {
  Local(&'a EpochView),
  Fork {
    client: &'a ForkClient,
    epoch_height: BlockHeight,
  },
}

impl NodeRuntime {
  /// Selects an executed eSpace block, including optimistic epochs. A block hash
  /// must identify an active pivot; known detached blocks never fall back remotely.
  pub(crate) fn espace_block_source(
    &self,
    selector: EspaceBlockSelector,
  ) -> Option<EspaceBlockSource<'_>> {
    let initial = &self.reset_state.initial_view;
    let height = match selector {
      EspaceBlockSelector::Epoch(epoch) => self.epoch_height(epoch),
      EspaceBlockSelector::Hash(hash) => {
        if hash == initial.state().epoch_id {
          initial.epoch_height()
        } else if let Some(block) = self.runtime_state.history.mined_block_by_hash(&hash) {
          if block.epoch_id() != hash {
            return None;
          }
          block.epoch_height()
        } else {
          if self.runtime_state.graph.block(&hash).is_some() {
            return None;
          }
          return initial.fork_client().map(|client| EspaceBlockSource::Fork {
            client,
            selector: HistoryBlockId::Hash(hash),
          });
        }
      }
    };

    if let Some(client) = initial.fork_client()
      && height <= client.base().epoch_height
    {
      return Some(EspaceBlockSource::Fork {
        client,
        selector: HistoryBlockId::Epoch(height),
      });
    }
    self
      .runtime_state
      .history
      .epoch_at_height(height)?
      .espace_block()
      .map(EspaceBlockSource::Local)
  }

  /// Pool and local identities shadow remote history, including other-Space entries.
  pub(crate) fn espace_transaction_source(
    &self,
    hash: H256,
  ) -> Option<EspaceTransactionSource<'_>> {
    if let Some(transaction) = self.pool_transaction_by_hash(hash) {
      return (transaction.space() == Space::Ethereum).then_some(EspaceTransactionSource::Local(
        LocalEspaceTransactionSource::Pool(transaction),
      ));
    }
    let history = &self.runtime_state.history;
    if let Some(transaction) = history.mined_espace_transaction_by_hash(&hash) {
      return Some(EspaceTransactionSource::Local(
        LocalEspaceTransactionSource::Mined(transaction),
      ));
    }
    if history.contains_mined_transaction(&hash) {
      return None;
    }
    self
      .reset_state
      .initial_view
      .fork_client()
      .map(|client| EspaceTransactionSource::Fork { client, hash })
  }

  /// Finds a pool entry, otherwise the last active local projection match.
  /// Phantom sender/nonce duplicates are valid; remote history is not scanned.
  pub(crate) fn espace_transaction_by_sender_and_nonce(
    &self,
    sender: Address,
    nonce: U256,
  ) -> Option<LocalEspaceTransactionSource<'_>> {
    if let Some(pooled) = self
      .runtime_state
      .transaction_pool
      .transactions_in_nonce_range(sender.with_evm_space(), nonce..=nonce)
      .next()
    {
      return Some(LocalEspaceTransactionSource::Pool(pooled.transaction()));
    }
    self
      .runtime_state
      .history
      .mined_espace_transaction_by_sender_and_nonce(sender, nonce)
      .map(LocalEspaceTransactionSource::Mined)
  }

  fn espace_state_source(
    &self,
    selector: EspaceStateSelector,
  ) -> Result<EspaceStateSource<'_>, EspaceStateQueryError> {
    let epoch = match selector {
      EspaceStateSelector::Epoch(epoch) => epoch,
      EspaceStateSelector::Hash {
        hash,
        require_pivot,
      } => EpochSelector::Number(self.espace_state_epoch_by_hash(hash, require_pivot)?),
    };
    let height = self.epoch_height(epoch);
    if let Some(client) = self.reset_state.initial_view.fork_client()
      && height < client.base().epoch_height
    {
      return Ok(EspaceStateSource::Fork {
        client,
        epoch_height: height,
      });
    }
    Ok(EspaceStateSource::Local(self.epoch_for_state(epoch)?))
  }

  fn espace_state_epoch_by_hash(
    &self,
    hash: H256,
    require_pivot: bool,
  ) -> Result<BlockHeight, EspaceStateQueryError> {
    let initial = &self.reset_state.initial_view;
    if hash == initial.state().epoch_id {
      return Ok(initial.epoch_height());
    }
    if let Some(block) = self.runtime_state.history.mined_block_by_hash(&hash) {
      if require_pivot && block.epoch_id() != hash {
        return Err(EspaceStateQueryError::UnknownBlock(hash));
      }
      return Ok(block.epoch_height());
    }
    if self.runtime_state.graph.block(&hash).is_some() {
      return Err(EspaceStateQueryError::UnknownBlock(hash));
    }
    if let Some(client) = initial.fork_client()
      && let Some(height) = client.history_epoch_by_hash(hash, require_pivot)?
    {
      return Ok(height);
    }
    Err(EspaceStateQueryError::UnknownBlock(hash))
  }

  /// Local state begins at the funded Fork Base; earlier fields use its fixed remote prefix.
  /// State selection preserves numeric visibility and the existing stable-tag rules.
  pub(crate) fn espace_balance_at(
    &self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<U256, EspaceStateQueryError> {
    match self.espace_state_source(selector)? {
      EspaceStateSource::Local(epoch) => {
        Ok(State::new(epoch.state().version.open_database())?.balance(&address.with_evm_space())?)
      }
      EspaceStateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.espace_balance_at(address, epoch_height)?),
    }
  }

  pub(crate) fn espace_nonce_at(
    &self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<U256, EspaceStateQueryError> {
    match self.espace_state_source(selector)? {
      EspaceStateSource::Local(epoch) => {
        Ok(State::new(epoch.state().version.open_database())?.nonce(&address.with_evm_space())?)
      }
      EspaceStateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.espace_nonce_at(address, epoch_height)?),
    }
  }

  pub(crate) fn espace_pending_nonce(
    &self,
    address: Address,
  ) -> Result<U256, EspaceStateQueryError> {
    Ok(self.pending_nonce(address.with_evm_space())?)
  }

  pub(crate) fn espace_code_at(
    &self,
    address: Address,
    selector: EspaceStateSelector,
  ) -> Result<Arc<Vec<u8>>, EspaceStateQueryError> {
    match self.espace_state_source(selector)? {
      EspaceStateSource::Local(epoch) => Ok(
        State::new(epoch.state().version.open_database())?
          .code(&address.with_evm_space())?
          .unwrap_or_default(),
      ),
      EspaceStateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.espace_code_at(address, epoch_height)?),
    }
  }

  pub(crate) fn espace_storage_at(
    &self,
    address: Address,
    position: H256,
    selector: EspaceStateSelector,
  ) -> Result<H256, EspaceStateQueryError> {
    match self.espace_state_source(selector)? {
      EspaceStateSource::Local(epoch) => {
        let value = State::new(epoch.state().version.open_database())?
          .storage_at(&address.with_evm_space(), position.as_bytes())?;
        Ok(H256::from_uint(&value))
      }
      EspaceStateSource::Fork {
        client,
        epoch_height,
      } => Ok(client.espace_storage_at(address, position, epoch_height)?),
    }
  }
}
