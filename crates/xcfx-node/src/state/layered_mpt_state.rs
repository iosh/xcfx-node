//! Adapts layered in-memory MPT state to Conflux StateTrait.

use std::sync::{Arc, OnceLock};

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_storage_types::{Error, MptKeyValue, Result, StateTrait};
use primitives::{EpochId, MptValue, SkipInputCheck, StorageKey, StorageKeyWithSpace};

use super::state_version::{CommittedStateVersion, StateCandidate, StateVersion};

enum StateLifecycle {
  Writable(StateCandidate),
  Prepared(Arc<StateVersion>),
  Committed,
}

pub(crate) struct LayeredMptState {
  lifecycle: StateLifecycle,
  committed_state: Arc<OnceLock<CommittedStateVersion>>,
}

pub(crate) struct CommittedStateReceiver {
  committed_state: Arc<OnceLock<CommittedStateVersion>>,
}

impl LayeredMptState {
  pub(crate) fn new(candidate: StateCandidate) -> (Self, CommittedStateReceiver) {
    let committed_state = Arc::new(OnceLock::new());

    let state = Self {
      lifecycle: StateLifecycle::Writable(candidate),
      committed_state: Arc::clone(&committed_state),
    };

    let receiver = CommittedStateReceiver { committed_state };

    (state, receiver)
  }

  fn read(&self, key: StorageKeyWithSpace<'_>) -> Option<&[u8]> {
    match &self.lifecycle {
      StateLifecycle::Writable(candidate) => candidate.get(key),
      StateLifecycle::Prepared(version) => version.get(key),
      StateLifecycle::Committed => panic!("state read is not allowed after commit"),
    }
  }

  fn visit_visible_prefix<'a>(
    &'a self,
    prefix: StorageKeyWithSpace<'_>,
    visitor: impl FnMut(Vec<u8>, &'a [u8]),
  ) {
    match &self.lifecycle {
      StateLifecycle::Writable(candidate) => candidate.visit_visible_prefix(prefix, visitor),
      StateLifecycle::Prepared(version) => version.visit_visible_prefix(prefix, visitor),
      StateLifecycle::Committed => panic!("state read is not allowed after commit"),
    }
  }

  fn writable_mut(&mut self) -> &mut StateCandidate {
    let StateLifecycle::Writable(candidate) = &mut self.lifecycle else {
      panic!("state mutation requires the writable lifecycle");
    };

    candidate
  }
}

impl CommittedStateReceiver {
  pub(crate) fn committed_state(&self) -> Option<CommittedStateVersion> {
    self.committed_state.get().cloned()
  }
}

impl StateTrait for LayeredMptState {
  fn get(&self, key: StorageKeyWithSpace) -> Result<Option<Box<[u8]>>> {
    Ok(self.read(key).map(Box::<[u8]>::from))
  }

  fn set(&mut self, key: StorageKeyWithSpace, value: Box<[u8]>) -> Result<()> {
    self.writable_mut().set(key, value);
    Ok(())
  }

  fn delete(&mut self, key: StorageKeyWithSpace) -> Result<()> {
    self.writable_mut().delete(key);
    Ok(())
  }

  fn delete_test_only(&mut self, key: StorageKeyWithSpace) -> Result<Option<Box<[u8]>>> {
    let value = match self.writable_mut().remove_current_entry(key) {
      MptValue::None => None,
      MptValue::TombStone => Some(Box::<[u8]>::default()),
      MptValue::Some(value) => Some(value),
    };

    Ok(value)
  }

  fn delete_all(&mut self, prefix: StorageKeyWithSpace) -> Result<Option<Vec<MptKeyValue>>> {
    let deleted = self.writable_mut().delete_prefix(prefix);

    Ok(if deleted.is_empty() {
      None
    } else {
      Some(deleted)
    })
  }
  fn read_all(&mut self, prefix: StorageKeyWithSpace) -> Result<Option<Vec<MptKeyValue>>> {
    let mut entries = Vec::new();

    self.visit_visible_prefix(prefix, |key, value| {
      entries.push((key, Box::<[u8]>::from(value)));
    });

    Ok(if entries.is_empty() {
      None
    } else {
      Some(entries)
    })
  }

  fn read_all_with_callback(
    &mut self,
    prefix: StorageKeyWithSpace,
    callback: &mut dyn FnMut(MptKeyValue),
    only_account_key: bool,
  ) -> Result<()> {
    self.visit_visible_prefix(prefix, |key, value| {
      if only_account_key
        && !matches!(
          StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&key).key,
          StorageKey::AccountKey(_)
        )
      {
        return;
      }

      callback((key, Box::<[u8]>::from(value)));
    });

    Ok(())
  }

  fn compute_state_root(&mut self) -> Result<StateRootWithAuxInfo> {
    let lifecycle = std::mem::replace(&mut self.lifecycle, StateLifecycle::Committed);

    let version = match lifecycle {
      StateLifecycle::Writable(candidate) => Arc::new(candidate.into_version()),
      StateLifecycle::Prepared(version) => version,
      StateLifecycle::Committed => {
        panic!("state root computation is not allowed after commit")
      }
    };

    let root = version.root_with_aux_info();
    self.lifecycle = StateLifecycle::Prepared(version);

    Ok(root)
  }

  fn get_state_root(&self) -> Result<StateRootWithAuxInfo> {
    match &self.lifecycle {
      StateLifecycle::Writable(_) => Err(Error::StateCommitWithoutMerkleHash),
      StateLifecycle::Prepared(version) => Ok(version.root_with_aux_info()),
      StateLifecycle::Committed => {
        panic!("state root access is not allowed after commit")
      }
    }
  }

  fn commit(&mut self, epoch_id: EpochId) -> Result<StateRootWithAuxInfo> {
    let version = match &self.lifecycle {
      StateLifecycle::Prepared(version) => Arc::clone(version),
      StateLifecycle::Writable(_) => {
        panic!("state commit requires a prepared state root")
      }
      StateLifecycle::Committed => panic!("state commit may only happen once"),
    };

    let root = version.root_with_aux_info();
    let committed_state = CommittedStateVersion { epoch_id, version };

    assert!(
      self.committed_state.set(committed_state).is_ok(),
      "committed state was already handed off"
    );

    self.lifecycle = StateLifecycle::Committed;

    Ok(root)
  }
}

#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc};

  use cfx_storage_types::StateTrait;
  use primitives::{DeltaMptKeyPadding, EpochId, StorageKey, StorageKeyWithSpace};

  use super::LayeredMptState;
  use crate::state::{
    delta_mpt::{DeltaMptEntries, DeltaMptVersion},
    snapshot_mpt::SnapshotMptVersion,
    state_version::{IntermediateDeltaLayer, StateCandidate, StateVersion},
  };

  fn storage_key<'a>(address: &'a [u8; 20], slot: &'a [u8; 32]) -> StorageKeyWithSpace<'a> {
    StorageKey::StorageKey {
      address_bytes: address,
      storage_key: slot,
    }
    .with_native_space()
  }

  fn storage_prefix(address: &[u8; 20]) -> StorageKeyWithSpace<'_> {
    StorageKey::StorageRootKey(address).with_native_space()
  }

  fn value(byte: u8) -> Box<[u8]> {
    vec![byte].into_boxed_slice()
  }

  #[test]
  fn state_trait_preserves_layered_reads_deletes_and_commit_identity() {
    let address = [0x11; 20];
    let current_slot = [0x01; 32];
    let hidden_slot = [0x02; 32];
    let candidate_slot = [0x03; 32];
    let snapshot_slot = [0x04; 32];

    let snapshot_entries = BTreeMap::from([
      (
        storage_key(&address, &current_slot).to_key_bytes(),
        value(0x11),
      ),
      (
        storage_key(&address, &hidden_slot).to_key_bytes(),
        value(0x12),
      ),
      (
        storage_key(&address, &snapshot_slot).to_key_bytes(),
        value(0x14),
      ),
    ]);
    let snapshot = Arc::new(SnapshotMptVersion::new(snapshot_entries));

    let mut intermediate_padding = DeltaMptKeyPadding::default();
    intermediate_padding.copy_from_slice(&[0x22; 32]);
    let mut intermediate_entries = DeltaMptEntries::default();
    intermediate_entries.set_value(
      storage_key(&address, &current_slot).to_delta_mpt_key_bytes(&intermediate_padding),
      value(0x21),
    );
    intermediate_entries.set_tombstone(
      storage_key(&address, &hidden_slot).to_delta_mpt_key_bytes(&intermediate_padding),
    );
    let intermediate = Arc::new(DeltaMptVersion::new(intermediate_entries));

    let current_padding =
      StorageKeyWithSpace::delta_mpt_padding(&snapshot.merkle_root(), &intermediate.merkle_root());
    let mut current_entries = DeltaMptEntries::default();
    current_entries.set_value(
      storage_key(&address, &current_slot).to_delta_mpt_key_bytes(&current_padding),
      value(0x31),
    );

    let parent = Arc::new(StateVersion::new(
      EpochId::from([0x51; 32]),
      snapshot,
      Some(IntermediateDeltaLayer::new(
        EpochId::from([0x52; 32]),
        intermediate_padding,
        intermediate,
      )),
      Arc::new(DeltaMptVersion::new(current_entries)),
    ));
    let (mut state, receiver) = LayeredMptState::new(StateCandidate::new(parent));

    state
      .set(storage_key(&address, &candidate_slot), value(0x33))
      .unwrap();

    let mut visible = state.read_all(storage_prefix(&address)).unwrap().unwrap();
    visible.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut expected_visible = vec![
      (
        storage_key(&address, &current_slot).to_key_bytes(),
        value(0x31),
      ),
      (
        storage_key(&address, &candidate_slot).to_key_bytes(),
        value(0x33),
      ),
      (
        storage_key(&address, &snapshot_slot).to_key_bytes(),
        value(0x14),
      ),
    ];
    expected_visible.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    assert_eq!(visible, expected_visible);

    assert_eq!(
      state
        .delete_test_only(storage_key(&address, &current_slot))
        .unwrap()
        .as_deref(),
      Some(&[0x31][..])
    );
    assert_eq!(
      state
        .get(storage_key(&address, &current_slot))
        .unwrap()
        .as_deref(),
      Some(&[0x21][..])
    );

    let mut deleted = state.delete_all(storage_prefix(&address)).unwrap().unwrap();
    deleted.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));

    let mut expected_deleted = vec![
      (
        storage_key(&address, &current_slot).to_key_bytes(),
        value(0x21),
      ),
      (
        storage_key(&address, &candidate_slot).to_key_bytes(),
        value(0x33),
      ),
      (
        storage_key(&address, &snapshot_slot).to_key_bytes(),
        value(0x14),
      ),
    ];
    expected_deleted.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    assert_eq!(deleted, expected_deleted);

    for slot in [&current_slot, &hidden_slot, &candidate_slot, &snapshot_slot] {
      assert_eq!(state.get(storage_key(&address, slot)).unwrap(), None);
    }

    assert_eq!(
      state
        .delete_test_only(storage_key(&address, &candidate_slot))
        .unwrap(),
      None
    );

    let removed_tombstone = state
      .delete_test_only(storage_key(&address, &current_slot))
      .unwrap()
      .expect("lower-layer value is hidden by a current tombstone");
    assert!(removed_tombstone.is_empty());
    assert_eq!(
      state
        .get(storage_key(&address, &current_slot))
        .unwrap()
        .as_deref(),
      Some(&[0x21][..])
    );
    state.delete(storage_key(&address, &current_slot)).unwrap();

    let removed_hidden_tombstone = state
      .delete_test_only(storage_key(&address, &hidden_slot))
      .unwrap()
      .expect("snapshot key remains represented by a current tombstone");
    assert!(removed_hidden_tombstone.is_empty());
    assert_eq!(
      state.get(storage_key(&address, &hidden_slot)).unwrap(),
      None
    );
    state.delete(storage_key(&address, &hidden_slot)).unwrap();

    let prepared_root = state.compute_state_root().unwrap();
    assert_eq!(state.compute_state_root().unwrap(), prepared_root);
    assert_eq!(state.get_state_root().unwrap(), prepared_root);
    assert_eq!(
      state.get(storage_key(&address, &current_slot)).unwrap(),
      None
    );
    assert_eq!(state.read_all(storage_prefix(&address)).unwrap(), None);

    let epoch_id = EpochId::from([0x61; 32]);
    assert_eq!(state.commit(epoch_id).unwrap(), prepared_root);

    let committed = receiver
      .committed_state()
      .expect("committed state is handed back to the caller");
    assert_eq!(committed.epoch_id, epoch_id);
    assert_eq!(committed.version.root_with_aux_info(), prepared_root);
    assert_eq!(
      committed.version.get(storage_key(&address, &current_slot)),
      None
    );
    assert_eq!(
      committed
        .version
        .get(storage_key(&address, &candidate_slot)),
      None
    );
  }
}
