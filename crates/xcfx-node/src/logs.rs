//! Log matching inputs, epoch projections, and reporting cursors.

use std::{ops::ControlFlow, sync::Arc};

use cfx_addr::Network;
use cfx_types::{H256, Space};
use primitives::{
  filter::{FilterError, LogFilter, LogFilterParams},
  log_entry::LocalizedLogEntry,
};
use thiserror::Error;

use crate::{
  chain::EpochView, fork::ForkReadError, runtime::ChainChange,
  runtime_transaction::RuntimeTransaction,
};

/// Normalized input plus the address network retained by Core's wire decoder.
pub(crate) struct LogFilterInput {
  filter: LogFilter,
  address_network: Option<Network>,
}

impl LogFilterInput {
  pub(crate) fn new(filter: LogFilter, address_network: Option<Network>) -> Self {
    Self {
      filter,
      address_network,
    }
  }

  pub(crate) fn into_filter(self, network: Network) -> Result<LogFilter, LogQueryError> {
    if let Some(actual) = self.address_network {
      if actual != network {
        return Err(LogQueryError::Network {
          expected: network,
          actual,
        });
      }
    }
    Ok(self.filter)
  }
}

#[derive(Debug, Error)]
pub(crate) enum LogQueryError {
  #[error(transparent)]
  Filter(#[from] FilterError),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
  #[error("address network {actual:?} does not match node network {expected:?}")]
  Network { expected: Network, actual: Network },
  #[error("Filter not found")]
  FilterNotFound,
}

pub(crate) fn network(id: u64) -> Network {
  match id {
    1 => Network::Test,
    1029 => Network::Main,
    id => Network::Id(id),
  }
}

/// Core queries count within each original block; incremental reports count
/// across the epoch. An eSpace virtual block spans one epoch in both cases.
#[derive(Clone, Copy)]
pub(crate) enum LogIndexScope {
  Block,
  Epoch,
}

/// Assigns positions before matching and clones only matching entries.
/// A consumer can stop projection without materializing the remaining logs.
pub(crate) fn visit_local_logs(
  view: &Arc<EpochView>,
  filter: &LogFilterParams,
  index_scope: LogIndexScope,
  mut visit: impl FnMut(LocalizedLogEntry) -> ControlFlow<()>,
) -> ControlFlow<()> {
  let Some(artifacts) = view.artifacts() else {
    return ControlFlow::Continue(());
  };
  match filter.space {
    Space::Native => {
      let mut log_index = 0;
      for block in artifacts.ordered_blocks() {
        if matches!(index_scope, LogIndexScope::Block) {
          log_index = 0;
        }
        for result in block.transaction_receipts() {
          for (transaction_log_index, entry) in result
            .receipt()
            .logs
            .iter()
            .filter(|entry| entry.space == Space::Native)
            .enumerate()
          {
            let index = log_index;
            log_index += 1;
            if !filter.matches(entry) {
              continue;
            }
            visit(LocalizedLogEntry {
              entry: entry.clone(),
              block_hash: block.runtime_block().hash(),
              epoch_number: view.epoch_height(),
              block_timestamp: Some(block.runtime_block().header().timestamp()),
              transaction_hash: result.transaction().hash(),
              transaction_index: result.block_transaction_index(),
              log_index: index,
              transaction_log_index,
            })?;
          }
        }
      }
    }
    Space::Ethereum => {
      let block = view
        .espace_block()
        .expect("local artifacts provide an eSpace projection");
      for transaction in block.transactions() {
        for (transaction_log_index, entry) in transaction.receipt().logs().iter().enumerate() {
          if !filter.matches(entry) {
            continue;
          }
          visit(LocalizedLogEntry {
            entry: entry.clone(),
            block_hash: block.hash(),
            epoch_number: view.epoch_height(),
            block_timestamp: Some(block.pivot_header().timestamp()),
            transaction_hash: transaction.hash(),
            transaction_index: transaction.transaction_index(),
            log_index: transaction.first_log_index() + transaction_log_index,
            transaction_log_index,
          })?;
        }
      }
    }
  }
  ControlFlow::Continue(())
}

/// A reporting position maintained by the Runtime's ordered chain changes.
/// The installation position prevents replay after a visibility-only retreat;
/// `first_reported` limits retractions to results observed by this consumer.
#[derive(Clone)]
pub(crate) struct EpochCursor {
  last_epoch: u64,
  first_reported: Option<u64>,
}

impl EpochCursor {
  pub(crate) fn new(last_epoch: u64) -> Self {
    Self {
      last_epoch,
      first_reported: None,
    }
  }

  pub(crate) fn next_epoch(&self) -> Option<u64> {
    self.last_epoch.checked_add(1)
  }

  pub(crate) fn report(&mut self, epoch: u64) -> bool {
    if epoch <= self.last_epoch {
      return false;
    }
    self.first_reported.get_or_insert(epoch);
    self.last_epoch = epoch;
    true
  }

  /// Returns only previously reported epochs, newest first, while the change
  /// still pins their old execution results.
  pub(crate) fn retract(&mut self, change: &ChainChange) -> Vec<Arc<EpochView>> {
    let removed = change
      .history()
      .removed()
      .iter()
      .rev()
      .filter(|view| {
        self
          .first_reported
          .is_some_and(|first| view.epoch_height() >= first)
          && view.epoch_height() <= self.last_epoch
      })
      .cloned()
      .collect::<Vec<_>>();
    if change
      .history()
      .removed()
      .first()
      .is_some_and(|view| view.epoch_height() <= self.last_epoch)
    {
      self.last_epoch = change.history().common_ancestor().epoch_height();
      if self
        .first_reported
        .is_some_and(|first| first > self.last_epoch)
      {
        self.first_reported = None;
      }
    }
    removed
  }
}

pub(crate) struct LogChanges {
  pub(crate) reverted_from: Option<u64>,
  pub(crate) removed: Vec<LocalizedLogEntry>,
  pub(crate) added: Vec<LocalizedLogEntry>,
}

pub(crate) enum FilterChanges {
  Hashes(Vec<H256>),
  EspaceTransactions(Vec<RuntimeTransaction>),
  Logs(LogChanges),
}

#[derive(Clone, Copy)]
pub(crate) enum PendingTransactionFilter {
  Hashes(Space),
  FullEspace,
}
