//! Layered in-memory Conflux state versions and candidates.

use std::{
  collections::{BTreeMap, HashSet},
  sync::Arc,
};

use cfx_internal_common::{StateRootAuxInfo, StateRootWithAuxInfo};
use primitives::{
  DeltaMptKeyPadding, EpochId, MERKLE_NULL_NODE, MptValue, NULL_EPOCH, SkipInputCheck, StateRoot,
  StorageKey, StorageKeyWithSpace,
};

use super::{
  delta_mpt::{CurrentDeltaCandidate, DeltaMptVersion},
  snapshot_mpt::SnapshotMptVersion,
  state_proof::StateProof,
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

  pub(crate) fn genesis_parent() -> Self {
    Self::new(
      NULL_EPOCH,
      Arc::new(SnapshotMptVersion::empty()),
      None,
      Arc::new(DeltaMptVersion::empty()),
    )
  }

  /// Rotates the three Conflux state layers at a snapshot boundary.
  pub(crate) fn rotate_snapshot(&self, parent_epoch_id: EpochId) -> Self {
    let (snapshot_epoch_id, snapshot) = match &self.intermediate {
      None => (self.snapshot_epoch_id, Arc::clone(&self.snapshot)),
      Some(intermediate) => {
        let mut snapshot_entries = self
          .snapshot
          .iter()
          .map(|(key, value)| (key.to_vec(), value.to_vec().into_boxed_slice()))
          .collect::<BTreeMap<_, _>>();

        for (delta_key, value) in intermediate.mpt.iter() {
          let snapshot_key = StorageKeyWithSpace::from_delta_mpt_key(delta_key).to_key_bytes();

          match value {
            MptValue::None => {
              unreachable!("stored Delta MPT entries always contain a value or tombstone")
            }
            MptValue::TombStone => {
              snapshot_entries.remove(&snapshot_key);
            }
            MptValue::Some(value) => {
              snapshot_entries.insert(snapshot_key, value.to_vec().into_boxed_slice());
            }
          }
        }

        (
          intermediate.epoch_id,
          Arc::new(SnapshotMptVersion::new(snapshot_entries)),
        )
      }
    };

    let intermediate = IntermediateDeltaLayer::new(
      parent_epoch_id,
      self.current_key_padding.clone(),
      Arc::clone(&self.current),
    );

    Self::new(
      snapshot_epoch_id,
      snapshot,
      Some(intermediate),
      Arc::new(DeltaMptVersion::empty()),
    )
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

  pub(crate) fn visit_visible_prefix<'a>(
    &'a self,
    access_key_prefix: StorageKeyWithSpace<'_>,
    mut visitor: impl FnMut(Vec<u8>, &'a [u8]),
  ) {
    let logical_prefix = access_key_prefix.to_key_bytes();
    let needs_logical_filter = matches!(access_key_prefix.key, StorageKey::AddressPrefixKey(_));
    let mut shadowed = HashSet::new();
    let current_prefix = access_key_prefix.to_delta_mpt_key_bytes(&self.current_key_padding);

    self
      .current
      .visit_prefix(&current_prefix, |physical_key, value| {
        visit_visible_delta_entry(
          physical_key,
          value,
          &logical_prefix,
          needs_logical_filter,
          &mut shadowed,
          &mut visitor,
        );
      });

    self.visit_visible_below_current_prefix(
      access_key_prefix,
      &logical_prefix,
      needs_logical_filter,
      &mut shadowed,
      &mut visitor,
    );
  }

  /// Returns the logical value together with its layered state proof.
  pub(crate) fn get_with_proof(&self, key: StorageKeyWithSpace<'_>) -> (Option<&[u8]>, StateProof) {
    let mut proof = StateProof::default();

    let current_key = key.to_delta_mpt_key_bytes(&self.current_key_padding);
    if self.current.merkle_root() != MERKLE_NULL_NODE {
      proof.current_delta_proof = Some(self.current.proof(&current_key));
    }

    match self.current.get(&current_key) {
      MptValue::Some(value) => return (Some(value), proof),
      MptValue::TombStone => return (None, proof),
      MptValue::None => {}
    }

    if let Some(intermediate) = &self.intermediate {
      let intermediate_key = key.to_delta_mpt_key_bytes(&intermediate.key_padding);

      if intermediate.mpt.merkle_root() != MERKLE_NULL_NODE {
        proof.intermediate_delta_proof = Some(intermediate.mpt.proof(&intermediate_key));
      }

      match intermediate.mpt.get(&intermediate_key) {
        MptValue::Some(value) => return (Some(value), proof),
        MptValue::TombStone => return (None, proof),
        MptValue::None => {}
      }
    }

    let snapshot_key = key.to_key_bytes();
    proof.snapshot_proof = Some(self.snapshot.proof(&snapshot_key));

    (self.snapshot.get(&snapshot_key), proof)
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

  fn visit_visible_below_current_prefix<'a>(
    &'a self,
    access_key_prefix: StorageKeyWithSpace<'_>,
    logical_prefix: &[u8],
    needs_logical_filter: bool,
    shadowed: &mut HashSet<Vec<u8>>,
    visitor: &mut impl FnMut(Vec<u8>, &'a [u8]),
  ) {
    if let Some(intermediate) = &self.intermediate {
      let intermediate_prefix = access_key_prefix.to_delta_mpt_key_bytes(&intermediate.key_padding);

      intermediate
        .mpt
        .visit_prefix(&intermediate_prefix, |physical_key, value| {
          visit_visible_delta_entry(
            physical_key,
            value,
            logical_prefix,
            needs_logical_filter,
            shadowed,
            visitor,
          );
        });
    }

    self.snapshot.visit_prefix(logical_prefix, |key, value| {
      if !shadowed.contains(key) {
        visitor(key.to_vec(), value);
      }
    });
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

fn visit_visible_delta_entry<'a>(
  physical_key: &[u8],
  value: MptValue<&'a [u8]>,
  logical_prefix: &[u8],
  needs_logical_filter: bool,
  shadowed: &mut HashSet<Vec<u8>>,
  visitor: &mut impl FnMut(Vec<u8>, &'a [u8]),
) {
  let logical_key = StorageKeyWithSpace::from_delta_mpt_key(physical_key).to_key_bytes();

  if needs_logical_filter && !logical_key.starts_with(logical_prefix) {
    return;
  }

  if shadowed.contains(&logical_key) {
    return;
  }

  match value {
    MptValue::None => unreachable!("visited Delta MPT entries are never missing"),
    MptValue::TombStone => {
      shadowed.insert(logical_key);
    }
    MptValue::Some(value) => {
      shadowed.insert(logical_key.clone());
      visitor(logical_key, value);
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

  pub(crate) fn visit_visible_prefix<'a>(
    &'a self,
    access_key_prefix: StorageKeyWithSpace<'_>,
    mut visitor: impl FnMut(Vec<u8>, &'a [u8]),
  ) {
    let logical_prefix = access_key_prefix.to_key_bytes();
    let needs_logical_filter = matches!(access_key_prefix.key, StorageKey::AddressPrefixKey(_));
    let mut shadowed = HashSet::new();
    let current_prefix = access_key_prefix.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self
      .current
      .visit_effective_prefix(&current_prefix, |physical_key, value| {
        visit_visible_delta_entry(
          physical_key,
          value,
          &logical_prefix,
          needs_logical_filter,
          &mut shadowed,
          &mut visitor,
        );
      });

    self.parent.visit_visible_below_current_prefix(
      access_key_prefix,
      &logical_prefix,
      needs_logical_filter,
      &mut shadowed,
      &mut visitor,
    );
  }

  pub(crate) fn set(&mut self, key: StorageKeyWithSpace<'_>, value: Box<[u8]>) {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self.current.set_value(current_key, value);
  }

  pub(crate) fn delete(&mut self, key: StorageKeyWithSpace<'_>) {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self.current.set_tombstone(current_key);
  }

  /// Physically removes only the current-delta entry without hiding lower layers.
  pub(crate) fn remove_current_entry(
    &mut self,
    key: StorageKeyWithSpace<'_>,
  ) -> MptValue<Box<[u8]>> {
    let current_key = key.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    self.current.remove_entry(current_key)
  }

  pub(crate) fn remove_current_prefix(
    &mut self,
    access_key_prefix: StorageKeyWithSpace<'_>,
  ) -> Vec<(Vec<u8>, MptValue<Box<[u8]>>)> {
    let logical_prefix = access_key_prefix.to_key_bytes();
    let needs_logical_filter = matches!(access_key_prefix.key, StorageKey::AddressPrefixKey(_));
    let current_prefix = access_key_prefix.to_delta_mpt_key_bytes(&self.parent.current_key_padding);

    let mut entries_to_remove = Vec::new();

    self
      .current
      .visit_effective_prefix(&current_prefix, |physical_key, _| {
        let logical_key = StorageKeyWithSpace::from_delta_mpt_key(physical_key).to_key_bytes();

        if needs_logical_filter && !logical_key.starts_with(&logical_prefix) {
          return;
        }

        entries_to_remove.push((physical_key.to_vec(), logical_key));
      });

    let mut removed_entries = Vec::with_capacity(entries_to_remove.len());

    for (physical_key, logical_key) in entries_to_remove {
      let value = match self.current.remove_entry(physical_key) {
        MptValue::None => {
          unreachable!("collected current Delta MPT entry disappeared before removal")
        }
        value => value,
      };

      removed_entries.push((logical_key, value));
    }

    removed_entries
  }

  /// Deletes every visible value while preserving Conflux's physical layer mutations.
  pub(crate) fn delete_prefix(
    &mut self,
    access_key_prefix: StorageKeyWithSpace<'_>,
  ) -> Vec<(Vec<u8>, Box<[u8]>)> {
    let removed_current = self.remove_current_prefix(access_key_prefix);
    let mut covered_keys = HashSet::with_capacity(removed_current.len());
    let mut deleted_entries = Vec::with_capacity(removed_current.len());

    for (key, value) in removed_current {
      match value {
        MptValue::None => {
          unreachable!("removed current Delta MPT entries always existed before removal")
        }

        MptValue::TombStone => {
          covered_keys.insert(key);
        }

        MptValue::Some(value) => {
          covered_keys.insert(key.clone());
          deleted_entries.push((key, value));
        }
      }
    }

    let logical_prefix = access_key_prefix.to_key_bytes();
    let needs_logical_filter = matches!(access_key_prefix.key, StorageKey::AddressPrefixKey(_));
    let mut lower_keys_to_hide = HashSet::new();

    // Production writes current tombstones for intermediate values and every
    // snapshot entry, including snapshots already hidden by an intermediate tombstone.
    if let Some(intermediate) = &self.parent.intermediate {
      let intermediate_prefix = access_key_prefix.to_delta_mpt_key_bytes(&intermediate.key_padding);

      intermediate
        .mpt
        .visit_prefix(&intermediate_prefix, |physical_key, value| {
          let logical_key = StorageKeyWithSpace::from_delta_mpt_key(physical_key).to_key_bytes();

          if needs_logical_filter && !logical_key.starts_with(&logical_prefix) {
            return;
          }

          match value {
            MptValue::None => {
              unreachable!("visited intermediate Delta MPT entries are never missing")
            }
            MptValue::TombStone => {
              covered_keys.insert(logical_key);
            }
            MptValue::Some(value) => {
              let was_uncovered = covered_keys.insert(logical_key.clone());
              lower_keys_to_hide.insert(logical_key.clone());

              if was_uncovered {
                deleted_entries.push((logical_key, Box::<[u8]>::from(value)));
              }
            }
          }
        });
    }

    self
      .parent
      .snapshot
      .visit_prefix(&logical_prefix, |key, value| {
        let was_uncovered = !covered_keys.contains(key);
        let logical_key = key.to_vec();

        lower_keys_to_hide.insert(logical_key.clone());

        if was_uncovered {
          deleted_entries.push((logical_key, Box::<[u8]>::from(value)));
        }
      });

    for logical_key in lower_keys_to_hide {
      let key = StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&logical_key);

      self.delete(key);
    }

    deleted_entries
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

  use hex_literal::hex;
  use primitives::{MerkleHash, StorageKey};

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
  fn candidate_derivation_preserves_layered_state_and_parent_identity() {
    let missing = [0x66; 20];
    let snapshot_only = [0x11; 20];
    let intermediate_value = [0x22; 20];
    let intermediate_deleted = [0x33; 20];
    let current_value = [0x44; 20];
    let current_deleted = [0x55; 20];

    let mut snapshot_entries = BTreeMap::new();
    for (address, byte) in [
      (&snapshot_only, 0x10),
      (&intermediate_value, 0x20),
      (&intermediate_deleted, 0x30),
      (&current_value, 0x40),
      (&current_deleted, 0x50),
    ] {
      snapshot_entries.insert(account_key(address).to_key_bytes(), value(byte));
    }
    let snapshot = Arc::new(SnapshotMptVersion::new(snapshot_entries));
    let snapshot_root = snapshot.merkle_root();

    // Snapshot rotation preserves the padding with which this MPT was built.
    let mut intermediate_padding = DeltaMptKeyPadding::default();
    intermediate_padding.copy_from_slice(&hex!(
      "cea634304116f4c3d75b56d555ab2cec9c499548f3ec6d2c697983d58c6204fa"
    ));
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
    assert_eq!(
      &current_padding[..],
      &hex!("24fc3c2d0e58226464a42e682f250aca39850de032e2f6559552f130e141eece"),
    );
    let mut current_entries = DeltaMptEntries::default();
    current_entries.set_value(delta_key(&current_value, &current_padding), value(0x41));
    current_entries.set_tombstone(delta_key(&current_deleted, &current_padding));
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

    let actual_state_root = StateRoot {
      snapshot_root,
      intermediate_delta_root: intermediate_root,
      delta_root: current_root,
    };
    // Generated by the cfx-storage State path at revision
    // a9b2a3773ff4c08cd7373502bc2d306a94341042 from the same layer entries.
    let oracle_state_root = StateRoot {
      snapshot_root: MerkleHash::from(hex!(
        "1cdd934b1e388610f90c3b83b5d735fb88a0f04e2b9ed54ce347d5d4e9cb18b2"
      )),
      intermediate_delta_root: MerkleHash::from(hex!(
        "a11f8b548b2006f1757dd71415a5085f44fba220de6ea497575c04e48d25af87"
      )),
      delta_root: MerkleHash::from(hex!(
        "515a1e5ff198466498f8f12cfc5e9db68053267656eb27589b58356dc5d83a0a"
      )),
    };
    let oracle_state_root_hash = MerkleHash::from(hex!(
      "bc2469018d337e2dfd3db91a48322a0891b94b33948114cc115aff9a81e0f5df"
    ));
    assert_eq!(actual_state_root, oracle_state_root);
    assert_eq!(
      actual_state_root.compute_state_root_hash(),
      oracle_state_root_hash
    );
    let parent_identity = parent.root_with_aux_info();

    assert_eq!(
      parent_identity,
      StateRootWithAuxInfo {
        aux_info: StateRootAuxInfo {
          snapshot_epoch_id,
          intermediate_epoch_id,
          maybe_intermediate_mpt_key_padding: Some(intermediate_padding.clone()),
          delta_mpt_key_padding: current_padding.clone(),
          state_root_hash: oracle_state_root_hash,
        },
        state_root: actual_state_root,
      }
    );

    let proof_cases: [(&[u8; 20], Option<&[u8]>, (bool, bool, bool)); 6] = [
      (&current_value, Some(&[0x41]), (true, false, false)),
      (&current_deleted, None, (true, false, false)),
      (&intermediate_value, Some(&[0x21]), (true, true, false)),
      (&intermediate_deleted, None, (true, true, false)),
      (&snapshot_only, Some(&[0x10]), (true, true, true)),
      (&missing, None, (true, true, true)),
    ];
    for (address, expected, expected_layers) in proof_cases {
      let key = account_key(address);
      let (actual, proof) = parent.get_with_proof(key);

      assert_eq!(parent.get(key), expected);
      assert_eq!(actual, expected);
      assert_eq!(
        (
          proof.current_delta_proof.is_some(),
          proof.intermediate_delta_proof.is_some(),
          proof.snapshot_proof.is_some(),
        ),
        expected_layers,
      );
      assert!(proof.is_valid_kv(key, expected, &parent_identity));

      let wrong_value = [0xff];
      let incorrect_claim = match expected {
        Some(_) => None,
        None => Some(wrong_value.as_slice()),
      };
      assert!(!proof.is_valid_kv(key, incorrect_claim, &parent_identity));
    }

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

    let deleted_key = account_key(&intermediate_value);
    let (deleted_value, deleted_proof) = child.get_with_proof(deleted_key);
    assert_eq!(deleted_value, None);
    assert_eq!(
      (
        deleted_proof.current_delta_proof.is_some(),
        deleted_proof.intermediate_delta_proof.is_some(),
        deleted_proof.snapshot_proof.is_some(),
      ),
      (true, false, false),
    );
    assert!(deleted_proof.is_valid_kv(deleted_key, None, &child.root_with_aux_info()));

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

  #[test]
  fn snapshot_rotation_preserves_state_and_migrates_layer_identity() {
    let updated = [0x61; 20];
    let deleted = [0x62; 20];
    let current_only = [0x63; 20];

    let snapshot = Arc::new(SnapshotMptVersion::new(
      [
        (account_key(&updated).to_key_bytes(), value(0x10)),
        (account_key(&deleted).to_key_bytes(), value(0x20)),
      ]
      .into_iter()
      .collect(),
    ));
    let snapshot_root = snapshot.merkle_root();

    let intermediate_padding =
      StorageKeyWithSpace::delta_mpt_padding(&MERKLE_NULL_NODE, &MERKLE_NULL_NODE);
    let mut intermediate_entries = DeltaMptEntries::default();
    intermediate_entries.set_value(delta_key(&updated, &intermediate_padding), value(0x11));
    intermediate_entries.set_tombstone(delta_key(&deleted, &intermediate_padding));
    let intermediate = Arc::new(DeltaMptVersion::new(intermediate_entries));
    let intermediate_root = intermediate.merkle_root();

    let current_padding =
      StorageKeyWithSpace::delta_mpt_padding(&snapshot_root, &intermediate_root);
    let mut current_entries = DeltaMptEntries::default();
    current_entries.set_value(delta_key(&updated, &current_padding), value(0x12));
    current_entries.set_value(delta_key(&current_only, &current_padding), value(0x30));
    let current = Arc::new(DeltaMptVersion::new(current_entries));
    let current_root = current.merkle_root();

    let intermediate_epoch_id = EpochId::from([0x71; 32]);
    let parent = StateVersion::new(
      EpochId::from([0x70; 32]),
      snapshot,
      Some(IntermediateDeltaLayer::new(
        intermediate_epoch_id,
        intermediate_padding,
        intermediate,
      )),
      current,
    );

    let expected_reads: [Option<&[u8]>; 3] = [Some(&[0x12]), None, Some(&[0x30])];

    assert_eq!(
      [
        parent.get(account_key(&updated)),
        parent.get(account_key(&deleted)),
        parent.get(account_key(&current_only)),
      ],
      expected_reads
    );

    let parent_epoch_id = EpochId::from([0x72; 32]);
    let rotated = parent.rotate_snapshot(parent_epoch_id);

    assert_eq!(
      [
        rotated.get(account_key(&updated)),
        rotated.get(account_key(&deleted)),
        rotated.get(account_key(&current_only)),
      ],
      expected_reads
    );

    let expected_snapshot = SnapshotMptVersion::new(
      [(account_key(&updated).to_key_bytes(), value(0x11))]
        .into_iter()
        .collect(),
    );
    let expected_snapshot_root = expected_snapshot.merkle_root();
    let expected_current_padding =
      StorageKeyWithSpace::delta_mpt_padding(&expected_snapshot_root, &current_root);
    let expected_state_root = StateRoot {
      snapshot_root: expected_snapshot_root,
      intermediate_delta_root: current_root,
      delta_root: MERKLE_NULL_NODE,
    };
    let expected_state_root_hash = expected_state_root.compute_state_root_hash();

    assert_eq!(
      rotated.root_with_aux_info(),
      StateRootWithAuxInfo {
        state_root: expected_state_root,
        aux_info: StateRootAuxInfo {
          snapshot_epoch_id: intermediate_epoch_id,
          intermediate_epoch_id: parent_epoch_id,
          maybe_intermediate_mpt_key_padding: Some(current_padding),
          delta_mpt_key_padding: expected_current_padding,
          state_root_hash: expected_state_root_hash,
        },
      }
    );

    let rotated_identity = rotated.root_with_aux_info();
    for (address, expected, expected_layers) in [
      (&updated, Some(&[0x12][..]), (false, true, false)),
      (&deleted, None, (false, true, true)),
    ] {
      let key = account_key(address);
      let (actual, proof) = rotated.get_with_proof(key);

      assert_eq!(actual, expected);
      assert_eq!(
        (
          proof.current_delta_proof.is_some(),
          proof.intermediate_delta_proof.is_some(),
          proof.snapshot_proof.is_some(),
        ),
        expected_layers,
      );
      assert!(proof.is_valid_kv(key, expected, &rotated_identity));
    }
  }
}
