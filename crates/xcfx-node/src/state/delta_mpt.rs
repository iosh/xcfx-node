//! In-memory Conflux Delta MPT versions and current-delta candidates.

use std::{collections::BTreeMap, sync::Arc};

use cfx_mpt::TrieProof;
use primitives::{MerkleHash, MptValue};

use super::canonical_mpt::{CanonicalMpt, MptEntry};

#[derive(Clone, Debug, Eq, PartialEq)]
enum DeltaMptValue {
  Tombstone,
  Present(Box<[u8]>),
}

impl DeltaMptValue {
  fn from_bytes(value: Box<[u8]>) -> Self {
    // Conflux MPT reserves the empty value encoding for tombstones.
    if value.is_empty() {
      Self::Tombstone
    } else {
      Self::Present(value)
    }
  }

  fn as_encoded_bytes(&self) -> &[u8] {
    match self {
      Self::Tombstone => &[],
      Self::Present(value) => value.as_ref(),
    }
  }
}

/// Final physical entries used to construct a Delta MPT.
///
/// Missing keys are not stored. Every stored key maps to either a tombstone or
/// a present value, and iteration follows physical key order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DeltaMptEntries {
  entries: BTreeMap<Vec<u8>, DeltaMptValue>,
}

impl DeltaMptEntries {
  pub(crate) fn set_value(&mut self, key: Vec<u8>, value: Box<[u8]>) {
    self.entries.insert(key, DeltaMptValue::from_bytes(value));
  }

  pub(crate) fn set_tombstone(&mut self, key: Vec<u8>) {
    self.entries.insert(key, DeltaMptValue::Tombstone);
  }

  pub(crate) fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
    match self.entries.get(key) {
      None => MptValue::None,
      Some(DeltaMptValue::Tombstone) => MptValue::TombStone,
      Some(DeltaMptValue::Present(value)) => MptValue::Some(value.as_ref()),
    }
  }

  pub(crate) fn iter(&self) -> impl Iterator<Item = (&[u8], MptValue<&[u8]>)> {
    self.entries.iter().map(|(key, value)| {
      let value = match value {
        DeltaMptValue::Tombstone => MptValue::TombStone,
        DeltaMptValue::Present(value) => MptValue::Some(value.as_ref()),
      };

      (key.as_slice(), value)
    })
  }

  fn encoded_entries(&self) -> impl Iterator<Item = MptEntry<'_>> {
    self.entries.iter().map(|(key, value)| MptEntry {
      key: key.as_slice(),
      value: value.as_encoded_bytes(),
    })
  }
}

/// One physical current-delta change relative to an immutable parent.
enum CurrentDeltaChange {
  Remove,
  Set(DeltaMptValue),
}

/// One complete, immutable Delta MPT version.
pub(crate) struct DeltaMptVersion {
  entries: DeltaMptEntries,
  mpt: CanonicalMpt,
}

impl DeltaMptVersion {
  pub(crate) fn new(entries: DeltaMptEntries) -> Self {
    let mpt = CanonicalMpt::build(entries.encoded_entries());
    Self { entries, mpt }
  }

  pub(crate) fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
    self.mpt.get(key)
  }

  pub(crate) fn merkle_root(&self) -> MerkleHash {
    self.mpt.merkle_root()
  }

  pub(crate) fn proof(&self, key: &[u8]) -> TrieProof {
    self.mpt.proof(key)
  }
}

/// Unpublished changes based on one immutable current-delta version.
pub(crate) struct CurrentDeltaCandidate {
  parent: Arc<DeltaMptVersion>,
  changes: BTreeMap<Vec<u8>, CurrentDeltaChange>,
}

impl CurrentDeltaCandidate {
  pub(crate) fn new(parent: Arc<DeltaMptVersion>) -> Self {
    Self {
      parent,
      changes: BTreeMap::new(),
    }
  }

  pub(crate) fn set_value(&mut self, key: Vec<u8>, value: Box<[u8]>) {
    self.changes.insert(
      key,
      CurrentDeltaChange::Set(DeltaMptValue::from_bytes(value)),
    );
  }

  pub(crate) fn set_tombstone(&mut self, key: Vec<u8>) {
    self
      .changes
      .insert(key, CurrentDeltaChange::Set(DeltaMptValue::Tombstone));
  }

  pub(crate) fn remove_entry(&mut self, key: Vec<u8>) {
    self.changes.insert(key, CurrentDeltaChange::Remove);
  }

  pub(crate) fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
    match self.changes.get(key) {
      None => self.parent.entries.get(key),
      Some(CurrentDeltaChange::Remove) => MptValue::None,
      Some(CurrentDeltaChange::Set(DeltaMptValue::Tombstone)) => MptValue::TombStone,
      Some(CurrentDeltaChange::Set(DeltaMptValue::Present(value))) => {
        MptValue::Some(value.as_ref())
      }
    }
  }

  pub(crate) fn into_version(self) -> DeltaMptVersion {
    let mut entries = self.parent.entries.clone();

    for (key, change) in self.changes {
      match change {
        CurrentDeltaChange::Remove => {
          entries.entries.remove(&key);
        }
        CurrentDeltaChange::Set(value) => {
          entries.entries.insert(key, value);
        }
      }
    }

    DeltaMptVersion::new(entries)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn entries_preserve_three_state_read_semantics() {
    let key = vec![0x10, 0x20];
    let mut entries = DeltaMptEntries::default();

    assert_eq!(entries.get(&key), MptValue::None);

    entries.set_value(key.clone(), vec![0xaa].into_boxed_slice());
    assert_eq!(entries.get(&key), MptValue::Some(&[0xaa][..]));

    entries.set_tombstone(key.clone());
    assert_eq!(entries.get(&key), MptValue::TombStone);

    entries.set_value(key.clone(), vec![0xbb].into_boxed_slice());
    assert_eq!(entries.get(&key), MptValue::Some(&[0xbb][..]));
  }

  #[test]
  fn iteration_is_key_ordered_and_contains_only_latest_values() {
    let mut entries = DeltaMptEntries::default();

    entries.set_value(vec![0x20], vec![0x02].into_boxed_slice());
    entries.set_tombstone(vec![0x01]);
    entries.set_value(vec![0x10], vec![0x01].into_boxed_slice());
    entries.set_value(vec![0x20], vec![0x03].into_boxed_slice());

    assert_eq!(
      entries.iter().collect::<Vec<_>>(),
      vec![
        (&[0x01][..], MptValue::TombStone),
        (&[0x10][..], MptValue::Some(&[0x01][..])),
        (&[0x20][..], MptValue::Some(&[0x03][..])),
      ]
    );
  }

  #[test]
  fn candidate_materialization_preserves_parent_and_sibling_isolation() {
    let mut entries = DeltaMptEntries::default();
    entries.set_value(vec![0x10], vec![0xaa].into_boxed_slice());
    entries.set_value(vec![0x20], vec![0xbb].into_boxed_slice());
    entries.set_value(vec![0x40], vec![0xdd].into_boxed_slice());

    let parent = Arc::new(DeltaMptVersion::new(entries));
    let parent_root = parent.merkle_root();
    let mut candidate = CurrentDeltaCandidate::new(Arc::clone(&parent));
    let mut sibling = CurrentDeltaCandidate::new(Arc::clone(&parent));

    candidate.set_value(vec![0x10], vec![0xcc].into_boxed_slice());
    candidate.remove_entry(vec![0x20]);
    candidate.set_tombstone(vec![0x30]);
    sibling.set_value(vec![0x10], vec![0xee].into_boxed_slice());

    assert_eq!(candidate.get(&[0x10]), MptValue::Some(&[0xcc][..]));
    assert_eq!(candidate.get(&[0x20]), MptValue::None);
    assert_eq!(candidate.get(&[0x30]), MptValue::TombStone);
    assert_eq!(candidate.get(&[0x40]), MptValue::Some(&[0xdd][..]));

    let child = candidate.into_version();

    assert_eq!(parent.merkle_root(), parent_root);
    assert_eq!(parent.get(&[0x10]), MptValue::Some(&[0xaa][..]));
    assert_eq!(parent.get(&[0x20]), MptValue::Some(&[0xbb][..]));

    assert_eq!(sibling.get(&[0x10]), MptValue::Some(&[0xee][..]));
    assert_eq!(sibling.get(&[0x20]), MptValue::Some(&[0xbb][..]));

    assert_eq!(child.get(&[0x10]), MptValue::Some(&[0xcc][..]));
    assert_eq!(child.get(&[0x20]), MptValue::None);
    assert_eq!(child.get(&[0x30]), MptValue::TombStone);
    assert_eq!(child.get(&[0x40]), MptValue::Some(&[0xdd][..]));
    assert_ne!(child.merkle_root(), parent_root);
  }
}
