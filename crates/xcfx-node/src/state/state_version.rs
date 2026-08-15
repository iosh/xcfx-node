//! Layered in-memory Conflux state versions and candidates.

use std::sync::Arc;

use cfx_internal_common::{StateRootAuxInfo, StateRootWithAuxInfo};
use primitives::{
  DeltaMptKeyPadding, EpochId, MERKLE_NULL_NODE, MptValue, NULL_EPOCH, StateRoot,
  StorageKeyWithSpace,
};

use super::{
  delta_mpt::{CurrentDeltaCandidate, DeltaMptVersion},
  snapshot_mpt::SnapshotMptVersion,
};

/// An intermediate Delta MPT with the epoch and padding used by its physical keys.
#[derive(Clone)]
pub(crate) struct IntermediateDeltaLayer {
  epoch_id: EpochId,
  key_padding: DeltaMptKeyPadding,
  mpt: Arc<DeltaMptVersion>,
}

impl IntermediateDeltaLayer {
  pub(crate) fn new(
    epoch_id: EpochId,
    key_padding: DeltaMptKeyPadding,
    mpt: Arc<DeltaMptVersion>,
  ) -> Self {
    Self {
      epoch_id,
      key_padding,
      mpt,
    }
  }
}

/// One immutable Conflux state composed from its three physical MPT layers.
pub(crate) struct StateVersion {
  snapshot_epoch_id: EpochId,
  snapshot: Arc<SnapshotMptVersion>,
  intermediate: Option<IntermediateDeltaLayer>,
  current_key_padding: DeltaMptKeyPadding,
  current: Arc<DeltaMptVersion>,
}

impl StateVersion {
  pub(crate) fn new(
    snapshot_epoch_id: EpochId,
    snapshot: Arc<SnapshotMptVersion>,
    intermediate: Option<IntermediateDeltaLayer>,
    current: Arc<DeltaMptVersion>,
  ) -> Self {
    let intermediate_root = intermediate
      .as_ref()
      .map_or(MERKLE_NULL_NODE, |layer| layer.mpt.merkle_root());

    let current_key_padding =
      StorageKeyWithSpace::delta_mpt_padding(&snapshot.merkle_root(), &intermediate_root);

    Self {
      snapshot_epoch_id,
      snapshot,
      intermediate,
      current_key_padding,
      current,
    }
  }

  pub(crate) fn get(&self, key: StorageKeyWithSpace<'_>) -> Option<&[u8]> {
    let current_key = key.to_delta_mpt_key_bytes(&self.current_key_padding);

    match self.current.get(&current_key) {
      MptValue::Some(value) => return Some(value),
      MptValue::TombStone => return None,
      MptValue::None => {}
    }

    self.get_below_current(&key)
  }

  fn get_below_current(&self, key: &StorageKeyWithSpace<'_>) -> Option<&[u8]> {
    if let Some(intermediate) = &self.intermediate {
      let intermediate_key = key.to_delta_mpt_key_bytes(&intermediate.key_padding);

      match intermediate.mpt.get(&intermediate_key) {
        MptValue::Some(value) => return Some(value),
        MptValue::TombStone => return None,
        MptValue::None => {}
      }
    }

    self.snapshot.get(&key.to_key_bytes())
  }

  pub(crate) fn root_with_aux_info(&self) -> StateRootWithAuxInfo {
    let intermediate_root = self
      .intermediate
      .as_ref()
      .map_or(MERKLE_NULL_NODE, |layer| layer.mpt.merkle_root());

    let state_root = StateRoot {
      snapshot_root: self.snapshot.merkle_root(),
      intermediate_delta_root: intermediate_root,
      delta_root: self.current.merkle_root(),
    };
    let state_root_hash = state_root.compute_state_root_hash();

    StateRootWithAuxInfo {
      state_root,
      aux_info: StateRootAuxInfo {
        snapshot_epoch_id: self.snapshot_epoch_id,
        intermediate_epoch_id: self
          .intermediate
          .as_ref()
          .map_or(NULL_EPOCH, |layer| layer.epoch_id),
        maybe_intermediate_mpt_key_padding: self
          .intermediate
          .as_ref()
          .map(|layer| layer.key_padding.clone()),
        delta_mpt_key_padding: self.current_key_padding.clone(),
        state_root_hash,
      },
    }
  }
}

/// Unpublished logical state changes based on one immutable parent version.
pub(crate) struct StateCandidate {
  parent: Arc<StateVersion>,
  current: CurrentDeltaCandidate,
}

impl StateCandidate {
  pub(crate) fn new(parent: Arc<StateVersion>) -> Self {
    let current = CurrentDeltaCandidate::new(Arc::clone(&parent.current));

    Self { parent, current }
  }

  pub(crate) fn get(&self, key: StorageKeyWithSpace<'_>) -> Option<&[u8]> {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    match self.current.get(&current_key) {
      MptValue::Some(value) => return Some(value),
      MptValue::TombStone => return None,
      MptValue::None => {}
    }

    self.parent.get_below_current(&key)
  }

  pub(crate) fn set(&mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>) {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self.current.set_value(current_key, value);
  }

  pub(crate) fn delete(&mut self, key: StorageKeyWithSpace<'_>) {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self.current.set_tombstone(current_key);
  }

  pub(crate) fn into_version(self) -> StateVersion {
    let Self { parent, current } = self;

    StateVersion::new(
      parent.snapshot_epoch_id,
      Arc::clone(&parent.snapshot),
      parent.intermediate.clone(),
      Arc::new(current.into_version()),
    )
  }
}

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap;

  use primitives::StorageKey;

  use super::*;
  use crate::state::delta_mpt::DeltaMptEntries;

  fn account_key(address: &[u8; 20]) -> StorageKeyWithSpace<'_> {
    StorageKey::AccountKey(address).with_native_space()
  }

  fn delta_key(address: &[u8; 20], padding: &DeltaMptKeyPadding) -> Vec<u8> {
    account_key(address).to_delta_mpt_key_bytes(padding)
  }

  fn value(byte: u8) -> Box<[u8]> {
    vec![byte].into_boxed_slice()
  }

  #[test]
  fn candidate_derivation_preserves_layered_reads_and_parent_identity() {
    let snapshot_only = [0x11; 20];
    let intermediate_value = [0x22; 20];
    let intermediate_deleted = [0x33; 20];
    let current_value = [0x44; 20];

    let mut snapshot_entries = BTreeMap::new();
    for (address, byte) in [
      (&snapshot_only, 0x10),
      (&intermediate_value, 0x20),
      (&intermediate_deleted, 0x30),
      (&current_value, 0x40),
    ] {
      snapshot_entries.insert(account_key(address).to_key_bytes(), value(byte));
    }
    let snapshot = Arc::new(SnapshotMptVersion::new(snapshot_entries));
    let snapshot_root = snapshot.merkle_root();

    let intermediate_padding =
      StorageKeyWithSpace::delta_mpt_padding(&MERKLE_NULL_NODE, &MERKLE_NULL_NODE);
    let mut intermediate_entries = DeltaMptEntries::default();
    intermediate_entries.set_value(
      delta_key(&intermediate_value, &intermediate_padding),
      value(0x21),
    );
    intermediate_entries.set_tombstone(delta_key(&intermediate_deleted, &intermediate_padding));
    let intermediate = Arc::new(DeltaMptVersion::new(intermediate_entries));
    let intermediate_root = intermediate.merkle_root();

    let current_padding =
      StorageKeyWithSpace::delta_mpt_padding(&snapshot_root, &intermediate_root);
    let mut current_entries = DeltaMptEntries::default();
    current_entries.set_value(delta_key(&current_value, &current_padding), value(0x41));
    let current = Arc::new(DeltaMptVersion::new(current_entries));
    let current_root = current.merkle_root();

    let snapshot_epoch_id = EpochId::from([0x51; 32]);
    let intermediate_epoch_id = EpochId::from([0x52; 32]);
    let parent = Arc::new(StateVersion::new(
      snapshot_epoch_id,
      snapshot,
      Some(IntermediateDeltaLayer::new(
        intermediate_epoch_id,
        intermediate_padding.clone(),
        intermediate,
      )),
      current,
    ));

    assert_eq!(parent.get(account_key(&snapshot_only)), Some(&[0x10][..]));
    assert_eq!(
      parent.get(account_key(&intermediate_value)),
      Some(&[0x21][..])
    );
    assert_eq!(parent.get(account_key(&intermediate_deleted)), None);
    assert_eq!(parent.get(account_key(&current_value)), Some(&[0x41][..]));

    let expected_state_root = StateRoot {
      snapshot_root,
      intermediate_delta_root: intermediate_root,
      delta_root: current_root,
    };
    let parent_identity = parent.root_with_aux_info();

    assert_eq!(
      parent_identity,
      StateRootWithAuxInfo {
        aux_info: StateRootAuxInfo {
          snapshot_epoch_id,
          intermediate_epoch_id,
          maybe_intermediate_mpt_key_padding: Some(intermediate_padding.clone()),
          delta_mpt_key_padding: current_padding.clone(),
          state_root_hash: expected_state_root.compute_state_root_hash(),
        },
        state_root: expected_state_root,
      }
    );

    let mut candidate = StateCandidate::new(Arc::clone(&parent));
    candidate.set(account_key(&snapshot_only), value(0x11));
    candidate.delete(account_key(&intermediate_value));

    assert_eq!(
      candidate.get(account_key(&snapshot_only)),
      Some(&[0x11][..])
    );
    assert_eq!(candidate.get(account_key(&intermediate_value)), None);

    let child = candidate.into_version();

    assert_eq!(child.get(account_key(&snapshot_only)), Some(&[0x11][..]));
    assert_eq!(child.get(account_key(&intermediate_value)), None);
    assert_eq!(parent.get(account_key(&snapshot_only)), Some(&[0x10][..]));
    assert_eq!(
      parent.get(account_key(&intermediate_value)),
      Some(&[0x21][..])
    );
    assert_eq!(parent.root_with_aux_info(), parent_identity);

    let child_identity = child.root_with_aux_info();
    assert_eq!(child_identity.state_root.snapshot_root, snapshot_root);
    assert_eq!(
      child_identity.state_root.intermediate_delta_root,
      intermediate_root
    );
    assert_eq!(
      child_identity.aux_info.delta_mpt_key_padding,
      current_padding
    );
    assert_ne!(
      child_identity.state_root.delta_root,
      parent_identity.state_root.delta_root
    );
  }
}
