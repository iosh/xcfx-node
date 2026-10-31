//! Resolves complete log queries against one active Runtime view.

use std::{
  collections::{BTreeMap, HashSet},
  ops::ControlFlow,
};

use cfx_addr::Network;
use cfx_rpc_cfx_types::Log as CoreLog;
use cfx_rpc_eth_types::Log as EspaceLog;
use cfx_types::{H256, Space, U256};
use primitives::{
  EpochNumber, LogEntry,
  filter::{FilterError, LogFilter, LogFilterParams},
  log_entry::LocalizedLogEntry,
};

use crate::{
  chain::EpochHistoryView,
  fork::{ForkReadError, HistoryBlockId},
  logs::{LogFilterInput, LogIndexScope, LogQueryError, network, visit_local_logs},
};

use super::{EpochSelector, NodeRuntime};

enum LogScope {
  Epochs {
    from: u64,
    to: u64,
  },
  Blocks(BTreeMap<u64, HashSet<H256>>),
  Numbers {
    from_epoch: u64,
    to_epoch: u64,
    first: H256,
    last: H256,
  },
}

impl NodeRuntime {
  pub(crate) fn prepare_log_filter(
    &self,
    input: LogFilterInput,
  ) -> Result<LogFilter, LogQueryError> {
    let filter = input.into_filter(network(self.network_id()))?;
    // eSpace resolves blockHash once at installation. Later getFilterLogs
    // queries that epoch's current pivot, as in Conflux's into_primitive.
    match filter {
      LogFilter::BlockHashLogFilter {
        block_hashes,
        params,
      } if params.space == Space::Ethereum => {
        let [hash] = block_hashes.as_slice() else {
          unreachable!("an eSpace blockHash filter selects exactly one pivot")
        };
        let epoch = self.log_epoch_by_hash(*hash, Space::Ethereum)?;
        Ok(LogFilter::EpochLogFilter {
          from_epoch: EpochNumber::Number(epoch),
          to_epoch: EpochNumber::Number(epoch),
          params,
        })
      }
      filter => Ok(filter),
    }
  }

  pub(crate) fn query_logs(
    &self,
    input: LogFilterInput,
  ) -> Result<Vec<LocalizedLogEntry>, LogQueryError> {
    let filter = self.prepare_log_filter(input)?;
    self.filtered_logs(&filter)
  }

  pub(crate) fn filtered_logs(
    &self,
    filter: &LogFilter,
  ) -> Result<Vec<LocalizedLogEntry>, LogQueryError> {
    let mut logs = Vec::new();
    match self.log_scope(filter)? {
      LogScope::Epochs { from, to } => {
        for epoch in from..=to {
          logs.extend(self.read_epoch_logs(epoch, filter, LogIndexScope::Block)?);
        }
      }
      LogScope::Blocks(epochs) => {
        for (epoch, hashes) in epochs {
          logs.extend(
            self
              .read_epoch_logs(epoch, filter, LogIndexScope::Block)?
              .into_iter()
              .filter(|log| filter.space == Space::Ethereum || hashes.contains(&log.block_hash)),
          );
        }
      }
      LogScope::Numbers {
        from_epoch,
        to_epoch,
        first,
        last,
      } => {
        for epoch in from_epoch..=to_epoch {
          let selected =
            if epoch == from_epoch || epoch == to_epoch {
              let hashes = self.log_block_hashes(epoch)?;
              let start =
                if epoch == from_epoch {
                  hashes.iter().position(|hash| *hash == first).ok_or(
                    ForkReadError::Inconsistent("log range start block membership"),
                  )?
                } else {
                  0
                };
              let end =
                if epoch == to_epoch {
                  hashes.iter().position(|hash| *hash == last).ok_or(
                    ForkReadError::Inconsistent("log range end block membership"),
                  )? + 1
                } else {
                  hashes.len()
                };
              if start > end {
                return Err(ForkReadError::Inconsistent("log block range order").into());
              }
              Some(hashes[start..end].iter().copied().collect::<HashSet<_>>())
            } else {
              None
            };
          logs.extend(
            self
              .read_epoch_logs(epoch, filter, LogIndexScope::Block)?
              .into_iter()
              .filter(|log| {
                selected
                  .as_ref()
                  .is_none_or(|hashes| hashes.contains(&log.block_hash))
              }),
          );
        }
      }
    }
    Ok(logs)
  }

  /// Reads one visible epoch with epoch-wide Core positions. Historical range
  /// fields apply to filtered_logs, not the incremental polling stream.
  pub(crate) fn poll_logs_in_epoch(
    &self,
    epoch: u64,
    params: &LogFilterParams,
  ) -> Result<Vec<LocalizedLogEntry>, LogQueryError> {
    self.read_epoch_logs(epoch, params, LogIndexScope::Epoch)
  }

  fn read_epoch_logs(
    &self,
    epoch: u64,
    params: &LogFilterParams,
    index_scope: LogIndexScope,
  ) -> Result<Vec<LocalizedLogEntry>, LogQueryError> {
    self.check_log_height(epoch)?;
    let mut logs = Vec::new();
    match self
      .epoch_history_at_height(epoch)
      .expect("a visible epoch belongs to the local or fixed remote history")
    {
      EpochHistoryView::Local(view) => {
        let _ = visit_local_logs(&view, params, index_scope, |log| {
          logs.push(log);
          ControlFlow::Continue(())
        });
      }
      EpochHistoryView::Fork {
        client,
        epoch_height,
      } => match params.space {
        Space::Native => {
          // The Fork boundary returns all Native logs in epoch execution order.
          // Derive polling positions before applying this consumer's predicates.
          let source_network = network(client.base().network.chain_ids.network_id);
          for (epoch_log_index, log) in client.core_logs(epoch_height)?.iter().enumerate() {
            let mut log = core_log(log, source_network)?;
            if matches!(index_scope, LogIndexScope::Epoch) {
              log.log_index = epoch_log_index;
            }
            if params.matches(&log.entry) {
              logs.push(log);
            }
          }
        }
        Space::Ethereum => {
          for log in client.espace_logs(epoch_height)?.iter() {
            let log = espace_log(log)?;
            if params.matches(&log.entry) {
              logs.push(log);
            }
          }
        }
      },
    }
    Ok(logs)
  }

  pub(crate) fn log_block_hashes(&self, epoch: u64) -> Result<Vec<H256>, LogQueryError> {
    self.check_log_height(epoch)?;
    match self
      .epoch_history_at_height(epoch)
      .expect("a visible epoch belongs to the local or fixed remote history")
    {
      EpochHistoryView::Local(view) => Ok(
        view
          .artifacts()
          .expect("local history has artifacts")
          .all_blocks()
          .map(|block| block.hash())
          .collect(),
      ),
      EpochHistoryView::Fork {
        client,
        epoch_height,
      } => Ok(client.block_hashes(epoch_height)?.to_vec()),
    }
  }

  fn check_log_height(&self, height: u64) -> Result<(), LogQueryError> {
    let max_epoch = self.epoch_height(EpochSelector::LatestState);
    if height > max_epoch {
      return Err(
        FilterError::OutOfBoundEpochNumber {
          to_epoch: height,
          max_epoch,
        }
        .into(),
      );
    }
    Ok(())
  }

  fn log_scope(&self, filter: &LogFilter) -> Result<LogScope, LogQueryError> {
    match filter {
      LogFilter::EpochLogFilter {
        from_epoch,
        to_epoch,
        ..
      } => {
        let from = self.log_epoch_selector(from_epoch);
        // Resolve on every query: an installed filter must retain its original
        // numeric upper bound as more epochs become visible.
        let to = match to_epoch {
          EpochNumber::Number(height) => {
            (*height).min(self.epoch_height(EpochSelector::LatestState))
          }
          selector => self.log_epoch_selector(selector),
        };
        if from > to {
          return Err(
            FilterError::InvalidEpochNumber {
              from_epoch: from,
              to_epoch: to,
            }
            .into(),
          );
        }
        self.check_log_height(to)?;
        Ok(LogScope::Epochs { from, to })
      }
      LogFilter::BlockHashLogFilter {
        block_hashes,
        params,
      } => {
        let mut epochs = BTreeMap::<u64, HashSet<H256>>::new();
        for &hash in block_hashes {
          let epoch = self.log_epoch_by_hash(hash, params.space)?;
          self.check_log_height(epoch)?;
          epochs.entry(epoch).or_default().insert(hash);
        }
        Ok(LogScope::Blocks(epochs))
      }
      LogFilter::BlockNumberLogFilter {
        from_block,
        to_block,
        ..
      } => {
        if from_block > to_block {
          return Err(
            FilterError::InvalidBlockNumber {
              from_block: *from_block,
              to_block: *to_block,
            }
            .into(),
          );
        }
        let max = self.latest_state_epoch().pivot_block_number();
        if *to_block > max {
          return Err(
            FilterError::Custom(format!(
              "block number {to_block} exceeds the latest visible block {max}"
            ))
            .into(),
          );
        }
        let (from_epoch, first) = self.log_block_by_number(*from_block)?;
        let (to_epoch, last) = self.log_block_by_number(*to_block)?;
        Ok(LogScope::Numbers {
          from_epoch,
          to_epoch,
          first,
          last,
        })
      }
    }
  }

  fn log_epoch_selector(&self, selector: &EpochNumber) -> u64 {
    self.epoch_height(match selector {
      EpochNumber::Number(number) => EpochSelector::Number(*number),
      EpochNumber::Earliest => EpochSelector::Earliest,
      EpochNumber::LatestCheckpoint => EpochSelector::Checkpoint,
      EpochNumber::LatestFinalized => EpochSelector::Finalized,
      EpochNumber::LatestConfirmed => EpochSelector::Confirmed,
      EpochNumber::LatestState => EpochSelector::LatestState,
      EpochNumber::LatestMined => EpochSelector::LatestMined,
    })
  }

  fn log_epoch_by_hash(&self, hash: H256, space: Space) -> Result<u64, LogQueryError> {
    if let Some(block) = self.mined_block_by_hash(&hash) {
      if space == Space::Ethereum && block.epoch_id() != hash {
        return Err(FilterError::UnknownBlock { hash }.into());
      }
      if block.block_number().is_none() {
        return Err(FilterError::BlockNotExecutedYet { block_hash: hash }.into());
      }
      return Ok(block.epoch_height());
    }
    if let Some(client) = self.reset_state.initial_view.fork_client() {
      if let Some(epoch) = client.history_epoch_by_hash(hash, space == Space::Ethereum)? {
        return Ok(epoch);
      }
    }
    Err(FilterError::UnknownBlock { hash }.into())
  }

  fn log_block_by_number(&self, number: u64) -> Result<(u64, H256), LogQueryError> {
    if let Some(block) = self.mined_block_by_number(number) {
      return Ok((block.epoch_height(), block.hash()));
    }
    if let Some(client) = self.reset_state.initial_view.fork_client() {
      if let Some(block) = client.core_block(HistoryBlockId::Number(number))? {
        let epoch = block
          .epoch_number
          .ok_or(ForkReadError::Unavailable("numbered block epoch"))?;
        return Ok((quantity_u64(epoch)?, block.hash));
      }
    }
    Err(FilterError::Custom(format!("unknown block number {number}")).into())
  }
}

fn quantity_u64(value: U256) -> Result<u64, ForkReadError> {
  if value > U256::from(u64::MAX) {
    return Err(ForkReadError::Inconsistent("log quantity range"));
  }
  Ok(value.as_u64())
}

fn index(value: Option<U256>) -> Result<usize, ForkReadError> {
  let value = value.ok_or(ForkReadError::Unavailable("log position"))?;
  usize::try_from(quantity_u64(value)?).map_err(|_| ForkReadError::Inconsistent("log index range"))
}

// Validate the remote encoding against the fixed source identity. RPC output
// uses the instance's network, which can be overridden independently.
fn core_log(log: &CoreLog, source_network: Network) -> Result<LocalizedLogEntry, ForkReadError> {
  if log.address.network != source_network || log.space.is_some_and(|space| space != Space::Native)
  {
    return Err(ForkReadError::Inconsistent(
      "Core log address network or space",
    ));
  }
  Ok(LocalizedLogEntry {
    entry: LogEntry {
      address: log.address.hex_address,
      topics: log.topics.clone(),
      data: log.data.0.clone(),
      space: Space::Native,
    },
    block_hash: log
      .block_hash
      .ok_or(ForkReadError::Unavailable("log block hash"))?,
    epoch_number: quantity_u64(
      log
        .epoch_number
        .ok_or(ForkReadError::Unavailable("log epoch"))?,
    )?,
    block_timestamp: log.block_timestamp.map(quantity_u64).transpose()?,
    transaction_hash: log
      .transaction_hash
      .ok_or(ForkReadError::Unavailable("log transaction hash"))?,
    transaction_index: index(log.transaction_index)?,
    log_index: index(log.log_index)?,
    transaction_log_index: index(log.transaction_log_index)?,
  })
}

fn espace_log(log: &EspaceLog) -> Result<LocalizedLogEntry, ForkReadError> {
  Ok(LocalizedLogEntry {
    entry: LogEntry {
      address: log.inner.address,
      topics: log.inner.topics.clone(),
      data: log.inner.data.0.clone(),
      space: Space::Ethereum,
    },
    block_hash: log.block_hash,
    epoch_number: log.block_number.as_u64(),
    block_timestamp: log.block_timestamp.map(|timestamp| timestamp.as_u64()),
    transaction_hash: log.transaction_hash,
    transaction_index: index(Some(log.transaction_index))?,
    log_index: index(log.log_index)?,
    transaction_log_index: index(log.transaction_log_index)?,
  })
}
