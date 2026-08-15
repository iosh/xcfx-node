//! Immutable in-memory Conflux snapshot MPT versions.

use std::collections::BTreeMap;

use primitives::MerkleHash;

use super::canonical_mpt::{CanonicalMpt, MptEntry};

/// One complete, immutable snapshot MPT version.
pub(crate) struct SnapshotMptVersion {
  entries: BTreeMap<Vec<u8>, Box<[u8]>>,
  mpt: CanonicalMpt,
}

impl SnapshotMptVersion {
  pub(crate) fn new(entries: BTreeMap<Vec<u8>, Box<[u8]>>) -> Self {
    let mpt = CanonicalMpt::build(entries.iter().map(|(key, value)| MptEntry {
      key: key.as_slice(),
      value: value.as_ref(),
    }));

    Self { entries, mpt }
  }

  pub(crate) fn get(&self, key: &[u8]) -> Option<&[u8]> {
    self.entries.get(key).map(Box::as_ref)
  }

  pub(crate) fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
    self
      .entries
      .iter()
      .map(|(key, value)| (key.as_slice(), value.as_ref()))
  }

  pub(crate) fn merkle_root(&self) -> MerkleHash {
    self.mpt.merkle_root()
  }
}
