//! Remote read cache with FIFO eviction.

use std::{
  collections::{HashMap, VecDeque},
  sync::Arc,
};

use cfx_types::{AddressWithSpace, U256};

use crate::execution::ExecutionCommitment;

use super::rpc::StateKey;

/// Retention limits for remote state and execution commitments.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ForkCacheConfig {
  max_entries: usize,
  /// Encoded value bytes, excluding collection overhead and process RSS.
  max_data_bytes: usize,
}

impl ForkCacheConfig {
  /// Zero in either field disables caching without disabling remote reads.
  pub(crate) const fn new(max_entries: usize, max_data_bytes: usize) -> Self {
    Self {
      max_entries,
      max_data_bytes,
    }
  }
}

impl Default for ForkCacheConfig {
  fn default() -> Self {
    Self::new(8192, 32 * 1024 * 1024)
  }
}

struct CacheEntry<T> {
  value: T,
  bytes: usize,
}

// Only eviction order combines the different kinds of cached data.
enum CacheKey {
  Balance(AddressWithSpace),
  StateValue(StateKey),
  Commitment(u64),
}

pub(super) struct ForkCache {
  balances: HashMap<AddressWithSpace, CacheEntry<U256>>,
  state_values: HashMap<StateKey, CacheEntry<Option<Arc<[u8]>>>>,
  commitments: HashMap<u64, CacheEntry<ExecutionCommitment>>,
  order: VecDeque<CacheKey>,
  bytes: usize,
  config: ForkCacheConfig,
}

impl ForkCache {
  pub(super) fn new(config: ForkCacheConfig) -> Self {
    Self {
      balances: HashMap::new(),
      state_values: HashMap::new(),
      commitments: HashMap::new(),
      order: VecDeque::new(),
      bytes: 0,
      config,
    }
  }

  pub(super) fn balance(&self, address: &AddressWithSpace) -> Option<U256> {
    self.balances.get(address).map(|entry| entry.value)
  }

  pub(super) fn state_value(&self, key: &StateKey) -> Option<Option<Arc<[u8]>>> {
    self.state_values.get(key).map(|entry| entry.value.clone())
  }

  pub(super) fn commitment(&self, epoch: u64) -> Option<ExecutionCommitment> {
    self
      .commitments
      .get(&epoch)
      .map(|entry| entry.value.clone())
  }

  pub(super) fn insert_balance(&mut self, address: AddressWithSpace, value: U256) {
    let bytes = 32;
    if self.balances.contains_key(&address) || !self.make_room(bytes) {
      return;
    }
    self.balances.insert(address, CacheEntry { value, bytes });
    self.order.push_back(CacheKey::Balance(address));
    self.bytes += bytes;
  }

  pub(super) fn insert_state_value(&mut self, key: StateKey, value: Option<Arc<[u8]>>) {
    let bytes = value.as_ref().map_or(0, |value| value.len());
    if self.state_values.contains_key(&key) || !self.make_room(bytes) {
      return;
    }
    self.state_values.insert(key, CacheEntry { value, bytes });
    self.order.push_back(CacheKey::StateValue(key));
    self.bytes += bytes;
  }

  pub(super) fn insert_commitment(&mut self, epoch: u64, value: ExecutionCommitment) {
    let bytes = 96;
    if self.commitments.contains_key(&epoch) || !self.make_room(bytes) {
      return;
    }
    self.commitments.insert(epoch, CacheEntry { value, bytes });
    self.order.push_back(CacheKey::Commitment(epoch));
    self.bytes += bytes;
  }

  fn make_room(&mut self, bytes: usize) -> bool {
    if self.config.max_entries == 0
      || self.config.max_data_bytes == 0
      || bytes > self.config.max_data_bytes
    {
      return false;
    }
    while self.order.len() >= self.config.max_entries
      || self.bytes > self.config.max_data_bytes - bytes
    {
      let oldest = self
        .order
        .pop_front()
        .expect("a full fork cache must contain an eviction entry");
      let removed_bytes = match oldest {
        CacheKey::Balance(address) => {
          self
            .balances
            .remove(&address)
            .expect("balance eviction must reference a cached balance")
            .bytes
        }
        CacheKey::StateValue(key) => {
          self
            .state_values
            .remove(&key)
            .expect("state eviction must reference a cached state entry")
            .bytes
        }
        CacheKey::Commitment(epoch) => {
          self
            .commitments
            .remove(&epoch)
            .expect("commitment eviction must reference a cached commitment")
            .bytes
        }
      };
      self.bytes -= removed_bytes;
    }
    true
  }
}
