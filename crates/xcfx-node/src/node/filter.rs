//! Polling state owned by the execution service, outside Runtime checkpoints.

use std::{
  collections::HashMap,
  ops::ControlFlow,
  sync::Arc,
  time::{Duration, Instant},
};

use cfx_types::{H128, H256, Space};
use primitives::{filter::LogFilter, log_entry::LocalizedLogEntry};

use crate::{
  chain::EpochView,
  logs::{
    EpochCursor, FilterChanges, LogChanges, LogFilterInput, LogIndexScope, LogQueryError,
    PendingTransactionFilter, visit_local_logs,
  },
  runtime::{ChainChange, EpochSelector, NodeRuntime},
  runtime_transaction::RuntimeTransaction,
};

pub(super) struct Filters {
  entries: HashMap<H128, Filter>,
  sequence: u64,
  idle_timeout: Duration,
}

struct Filter {
  kind: FilterKind,
  expires: Option<Instant>,
}

enum FilterKind {
  Logs {
    filter: LogFilter,
    cursor: EpochCursor,
    removed: Vec<Arc<EpochView>>,
  },
  Blocks {
    space: Space,
    cursor: EpochCursor,
  },
  Pending {
    space: Space,
    hashes: Vec<H256>,
  },
  PendingEspace {
    transactions: Vec<RuntimeTransaction>,
  },
}

impl FilterKind {
  fn space(&self) -> Space {
    match self {
      Self::Logs { filter, .. } => filter.space,
      Self::Blocks { space, .. } | Self::Pending { space, .. } => *space,
      Self::PendingEspace { .. } => Space::Ethereum,
    }
  }
}

impl Filters {
  pub(super) fn new(idle_timeout: Duration) -> Self {
    Self {
      entries: HashMap::new(),
      sequence: 0,
      idle_timeout,
    }
  }

  pub(super) fn next_expiration(&self) -> Option<Instant> {
    self
      .entries
      .values()
      .filter_map(|filter| filter.expires)
      .min()
  }

  pub(super) fn evict(&mut self) {
    let now = Instant::now();
    self
      .entries
      .retain(|_, filter| filter.expires.is_none_or(|expiry| expiry > now));
  }

  fn insert(&mut self, kind: FilterKind) -> H128 {
    self.evict();
    self.sequence = self
      .sequence
      .checked_add(1)
      .expect("filter ID space exhausted");
    let id = H128::from_low_u64_be(self.sequence);
    self.entries.insert(
      id,
      Filter {
        kind,
        expires: Instant::now().checked_add(self.idle_timeout),
      },
    );
    id
  }

  fn get(&mut self, space: Space, id: H128) -> Result<&mut FilterKind, LogQueryError> {
    self.evict();
    let filter = self
      .entries
      .get_mut(&id)
      .filter(|filter| filter.kind.space() == space)
      .ok_or(LogQueryError::FilterNotFound)?;
    filter.expires = Instant::now().checked_add(self.idle_timeout);
    Ok(&mut filter.kind)
  }

  pub(super) fn new_logs(
    &mut self,
    runtime: &NodeRuntime,
    input: LogFilterInput,
  ) -> Result<H128, LogQueryError> {
    let filter = runtime.prepare_log_filter(input)?;
    let cursor = EpochCursor::new(
      runtime
        .epoch_height(EpochSelector::LatestState)
        .saturating_sub(1),
    );
    Ok(self.insert(FilterKind::Logs {
      filter,
      cursor,
      removed: Vec::new(),
    }))
  }

  pub(super) fn new_blocks(&mut self, runtime: &NodeRuntime, space: Space) -> H128 {
    let cursor = EpochCursor::new(runtime.epoch_height(EpochSelector::LatestState));
    self.insert(FilterKind::Blocks { space, cursor })
  }

  pub(super) fn new_pending(&mut self, kind: PendingTransactionFilter) -> H128 {
    self.insert(match kind {
      PendingTransactionFilter::Hashes(space) => FilterKind::Pending {
        space,
        hashes: Vec::new(),
      },
      PendingTransactionFilter::FullEspace => FilterKind::PendingEspace {
        transactions: Vec::new(),
      },
    })
  }

  pub(super) fn uninstall(&mut self, space: Space, id: H128) -> bool {
    self.evict();
    if self
      .entries
      .get(&id)
      .is_none_or(|filter| filter.kind.space() != space)
    {
      return false;
    }
    self.entries.remove(&id).is_some()
  }

  pub(super) fn logs(
    &mut self,
    runtime: &NodeRuntime,
    space: Space,
    id: H128,
  ) -> Result<Vec<LocalizedLogEntry>, LogQueryError> {
    let FilterKind::Logs { filter, .. } = self.get(space, id)? else {
      return Err(LogQueryError::FilterNotFound);
    };
    runtime.filtered_logs(filter)
  }

  pub(super) fn changes(
    &mut self,
    runtime: &NodeRuntime,
    space: Space,
    id: H128,
  ) -> Result<FilterChanges, LogQueryError> {
    match self.get(space, id)? {
      FilterKind::Pending { hashes, .. } => Ok(FilterChanges::Hashes(std::mem::take(hashes))),
      FilterKind::PendingEspace { transactions } => Ok(FilterChanges::EspaceTransactions(
        std::mem::take(transactions),
      )),
      FilterKind::Blocks { cursor, space } => {
        let latest = runtime.epoch_height(EpochSelector::LatestState);
        let mut next = cursor.clone();
        let mut hashes = Vec::new();
        if let Some(start) = cursor.next_epoch() {
          for epoch in start..=latest {
            let view = runtime
              .epoch_at_height(epoch)
              .expect("a polling cursor stays within retained history");
            if next.report(epoch) {
              match space {
                Space::Native => hashes.extend(runtime.log_block_hashes(epoch)?),
                Space::Ethereum => hashes.push(view.state().epoch_id),
              }
            }
          }
        }
        *cursor = next;
        Ok(FilterChanges::Hashes(hashes))
      }
      FilterKind::Logs {
        filter,
        cursor,
        removed,
      } => {
        let latest = runtime.epoch_height(EpochSelector::LatestState);
        let mut next = cursor.clone();
        let mut added = Vec::new();
        if let Some(start) = cursor.next_epoch() {
          for epoch in start..=latest {
            if next.report(epoch) {
              added.extend(runtime.poll_logs_in_epoch(epoch, filter)?);
            }
          }
        }
        // The rewind can cross the installation position or happen repeatedly
        // before a poll. Core needs the first epoch of that rewound suffix,
        // including epochs preceding the first result this filter reported.
        let reverted_from = (!removed.is_empty()).then(|| {
          cursor
            .next_epoch()
            .expect("a cursor with retracted epochs has a successor")
        });
        let mut removed_logs = Vec::new();
        if filter.space == Space::Ethereum {
          for view in removed.iter() {
            let _ = visit_local_logs(view, filter, LogIndexScope::Epoch, |log| {
              removed_logs.push(log);
              ControlFlow::Continue(())
            });
          }
        }
        // Nothing is consumed until all state/remote reads have succeeded.
        *cursor = next;
        removed.clear();
        Ok(FilterChanges::Logs(LogChanges {
          reverted_from,
          removed: removed_logs,
          added,
        }))
      }
    }
  }

  pub(super) fn chain_changed(&mut self, change: &ChainChange) {
    self.evict();
    for filter in self.entries.values_mut() {
      match &mut filter.kind {
        FilterKind::Logs {
          cursor, removed, ..
        } => removed.extend(cursor.retract(change)),
        FilterKind::Blocks { cursor, .. } => {
          cursor.retract(change);
        }
        FilterKind::Pending { .. } | FilterKind::PendingEspace { .. } => {}
      }
    }
  }

  pub(super) fn transaction_inserted(&mut self, transaction: &RuntimeTransaction) {
    self.evict();
    for filter in self.entries.values_mut() {
      match &mut filter.kind {
        FilterKind::Pending { space, hashes } if *space == transaction.space() => {
          hashes.push(transaction.hash())
        }
        FilterKind::PendingEspace { transactions } if transaction.space() == Space::Ethereum => {
          transactions.push(transaction.clone())
        }
        _ => {}
      }
    }
  }
}
