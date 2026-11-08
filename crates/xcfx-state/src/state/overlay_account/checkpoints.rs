use crate::database::StorageEntry;
use crate::database::WriteCacheItem;
use std::{
  collections::{hash_map, HashMap},
  fmt::Debug,
  hash::Hash,
};

use cfx_types::U256;

use crate::{
  state::checkpoints::CheckpointEntry::{self, Recorded, Unchanged},
  unwrap_or_return,
};

use super::OverlayAccount;

#[derive(Debug, Clone)]
pub struct WriteCheckpointLayer<K: Hash + Clone + Eq, T: Clone> {
  storage_write: HashMap<K, CheckpointEntry<T>>,
  state_checkpoint_id: usize,
}

impl<K: Hash + Clone + Eq, T: Clone> Default for WriteCheckpointLayer<K, T> {
  fn default() -> Self {
    Self {
      storage_write: Default::default(),
      state_checkpoint_id: Default::default(),
    }
  }
}

impl<K: Hash + Clone + Eq, T: Clone> WriteCheckpointLayer<K, T> {
  pub(super) fn new_empty(state_checkpoint_id: usize) -> Self {
    Self {
      storage_write: HashMap::new(),
      state_checkpoint_id,
    }
  }

  fn revert_checkpoint(self, state_checkpoint_id: usize, cache: &mut HashMap<K, T>) {
    if self.state_checkpoint_id >= state_checkpoint_id {
      for (k, v) in self.storage_write.into_iter() {
        match v {
          Unchanged => cache.remove(&k),
          Recorded(storage_value) => cache.insert(k, storage_value),
        };
      }
    }
  }

  fn notify_cache_change(&mut self, key: K, old_value: Option<T>) {
    self
      .storage_write
      .entry(key)
      .or_insert(CheckpointEntry::from_cache(old_value));
  }
}

impl OverlayAccount {
  pub(super) fn insert_storage_write_cache(&mut self, key: Vec<u8>, value: StorageEntry) {
    let old_value = self
      .storage_write_cache
      .write()
      .insert(key.clone(), WriteCacheItem::Write(value));
    unwrap_or_return!(self.storage_write_checkpoint.as_mut())
      .get_mut()
      .notify_cache_change(key, old_value);
  }

  pub(super) fn mark_storage_warm(&self, key: &[u8]) {
    if let hash_map::Entry::Vacant(v) = self.storage_write_cache.write().entry(key.to_vec()) {
      v.insert(WriteCacheItem::Read);
      unwrap_or_return!(self.storage_write_checkpoint.as_ref())
        .write()
        .notify_cache_change(key.to_vec(), None);
    }
  }

  pub(super) fn insert_transient_write_cache(&mut self, key: Vec<u8>, value: U256) {
    let old_value = self
      .transient_storage_cache
      .write()
      .insert(key.clone(), value);
    unwrap_or_return!(self.transient_storage_checkpoint.as_mut())
      .notify_cache_change(key, old_value);
  }

  pub fn clear_checkpoint(&mut self) {
    self.storage_write_checkpoint = None;
    self.transient_storage_checkpoint = None;
  }

  pub fn revert_checkpoint(&mut self, state_checkpoint_id: usize) {
    if let Some(ct) = self.storage_write_checkpoint.take() {
      ct.into_inner()
        .revert_checkpoint(state_checkpoint_id, &mut self.storage_write_cache.write())
    };

    if let Some(ct) = self.transient_storage_checkpoint.take() {
      ct.revert_checkpoint(
        state_checkpoint_id,
        &mut self.transient_storage_cache.write(),
      )
    };
  }
}
