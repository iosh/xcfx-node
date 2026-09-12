//! Conflux state storage access over private MPT candidates.

use cfx_storage_types::{MptKeyValue, Result, StateStorage};
use primitives::{MptValue, SkipInputCheck, StorageKey, StorageKeyWithSpace};

use super::state_version::StateCandidate;

impl StateStorage for StateCandidate {
  fn get(&self, key: StorageKeyWithSpace<'_>) -> Result<Option<Box<[u8]>>> {
    Ok(StateCandidate::get(self, key).map(Box::<[u8]>::from))
  }

  fn set(&mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>) -> Result<()> {
    StateCandidate::set(self, key, value);
    Ok(())
  }

  fn delete(&mut self, key: StorageKeyWithSpace<'_>) -> Result<()> {
    StateCandidate::delete(self, key);
    Ok(())
  }

  fn delete_test_only(&mut self, key: StorageKeyWithSpace<'_>) -> Result<Option<Box<[u8]>>> {
    let value = match self.remove_current_entry(key) {
      MptValue::None => None,
      MptValue::TombStone => Some(Box::<[u8]>::default()),
      MptValue::Some(value) => Some(value),
    };

    Ok(value)
  }

  fn delete_all(&mut self, prefix: StorageKeyWithSpace<'_>) -> Result<Option<Vec<MptKeyValue>>> {
    let deleted = self.delete_prefix(prefix);

    Ok(if deleted.is_empty() {
      None
    } else {
      Some(deleted)
    })
  }

  fn read_all(&mut self, prefix: StorageKeyWithSpace<'_>) -> Result<Option<Vec<MptKeyValue>>> {
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
    prefix: StorageKeyWithSpace<'_>,
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
}

#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc};

  use cfx_storage_types::StateStorage;
  use primitives::{DeltaMptKeyPadding, EpochId, StorageKey, StorageKeyWithSpace};

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
  fn state_storage_preserves_layered_reads_and_deletes() {
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
    let mut candidate = StateCandidate::new(parent);
    let state: &mut dyn StateStorage = &mut candidate;

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

    assert_eq!(state.read_all(storage_prefix(&address)).unwrap(), None);

    let version = candidate.into_version();
    assert_eq!(version.get(storage_key(&address, &current_slot)), None);
    assert_eq!(version.get(storage_key(&address, &candidate_slot)), None);
  }
}
