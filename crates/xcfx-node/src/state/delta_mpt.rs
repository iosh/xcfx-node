//! In-memory Conflux Delta MPT versions and current-delta candidates.

use std::{collections::BTreeMap, ops::Range, sync::Arc};

use cfx_mpt::{
  CompressedPathRaw, CompressedPathTrait, TrieNodeTrait, TrieProof, TrieProofNode,
  VanillaChildrenTable, VanillaTrieNode,
  children_table::CHILDREN_COUNT,
  merkle::compute_merkle,
  walk::{GetChildTrait, WalkStop, walk},
};

use cfx_storage_types::access_mode;

use primitives::{MERKLE_NULL_NODE, MerkleHash, MptValue};

#[derive(Clone, Debug, Eq, PartialEq)]
enum DeltaMptValue {
  Tombstone,
  Present(Box<[u8]>),
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
    self.entries.insert(key, DeltaMptValue::Present(value));
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
}

/// One physical current-delta change relative to an immutable parent.
enum CurrentDeltaChange {
  Remove,
  Set(DeltaMptValue),
}

/// One complete, immutable Delta MPT version.
pub(crate) struct DeltaMptVersion {
  entries: DeltaMptEntries,
  mpt: CanonicalDeltaMpt,
}

impl DeltaMptVersion {
  pub(crate) fn new(entries: DeltaMptEntries) -> Self {
    let mpt = CanonicalDeltaMpt::build(&entries);
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
    self
      .changes
      .insert(key, CurrentDeltaChange::Set(DeltaMptValue::Present(value)));
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

fn nibble_len(key: &[u8]) -> usize {
  key.len() * 2
}

fn nibble_at(key: &[u8], nibble_index: usize) -> u8 {
  let byte = key[nibble_index / 2];

  if nibble_index.is_multiple_of(2) {
    byte >> 4
  } else {
    byte & 0x0f
  }
}

fn common_nibble_prefix_end(first: &[u8], last: &[u8], start: usize) -> usize {
  let end = nibble_len(first).min(nibble_len(last));
  let mut cursor = start;

  while cursor < end && nibble_at(first, cursor) == nibble_at(last, cursor) {
    cursor += 1;
  }

  cursor
}

fn compressed_path_from_key(key: &[u8], nibble_range: Range<usize>) -> CompressedPathRaw {
  let mut path_mask = CompressedPathRaw::NO_MISSING_NIBBLE;

  if !nibble_range.start.is_multiple_of(2) {
    path_mask |= CompressedPathRaw::first_nibble_mask();
  }

  if !nibble_range.end.is_multiple_of(2) {
    path_mask |= CompressedPathRaw::second_nibble_mask();
  }

  let byte_start = nibble_range.start / 2;
  let byte_end = nibble_range.end.div_ceil(2);

  CompressedPathRaw::new_and_apply_mask(&key[byte_start..byte_end], path_mask)
}

/// Identifies one node occurrence within a single in-memory trie.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NodeId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChildLink {
  child_index: u8,
  node_id: NodeId,
}

/// Couples a Conflux protocol node with its in-memory child occurrences.
struct StoredNode {
  protocol_node: VanillaTrieNode<MerkleHash>,
  child_links: Box<[ChildLink]>,
}

impl StoredNode {
  fn new(protocol_node: VanillaTrieNode<MerkleHash>, child_links: Vec<ChildLink>) -> Self {
    Self {
      protocol_node,
      child_links: child_links.into_boxed_slice(),
    }
  }

  fn child(&self, child_index: u8) -> Option<NodeId> {
    self
      .child_links
      .iter()
      .find(|link| link.child_index == child_index)
      .map(|link| link.node_id)
  }

  fn proof_node(&self, path_without_first_nibble: bool) -> TrieProofNode {
    TrieProofNode::new(
      self.protocol_node.get_children_table_ref().clone(),
      self
        .protocol_node
        .value_as_slice()
        .into_option()
        .map(|value| value.into()),
      self.protocol_node.compressed_path_ref().into(),
      path_without_first_nibble,
    )
  }
}

impl<'node> GetChildTrait<'node> for StoredNode {
  type ChildIdType = NodeId;

  fn get_child(&'node self, child_index: u8) -> Option<Self::ChildIdType> {
    self.child(child_index)
  }
}

/// Owns every node occurrence in one in-memory trie.
#[derive(Default)]
struct NodeArena {
  nodes: Vec<StoredNode>,
}

impl NodeArena {
  fn insert(&mut self, node: StoredNode) -> NodeId {
    let node_id = NodeId(self.nodes.len());
    self.nodes.push(node);
    node_id
  }

  fn node(&self, node_id: NodeId) -> &StoredNode {
    &self.nodes[node_id.0]
  }
}

type EntryRef<'a> = (&'a [u8], &'a DeltaMptValue);

/// Canonical in-memory Delta MPT built from one complete entry set.
struct CanonicalDeltaMpt {
  arena: NodeArena,
  root: NodeId,
}

impl CanonicalDeltaMpt {
  fn build(entries: &DeltaMptEntries) -> Self {
    let ordered_entries = entries
      .entries
      .iter()
      .map(|(key, value)| (key.as_slice(), value))
      .collect::<Vec<_>>();

    let mut arena = NodeArena::default();
    let root = if ordered_entries.is_empty() {
      arena.insert(StoredNode::new(VanillaTrieNode::default(), Vec::new()))
    } else {
      build_node(&mut arena, &ordered_entries, 0, true).0
    };

    Self { arena, root }
  }

  fn merkle_root(&self) -> MerkleHash {
    *self.arena.node(self.root).protocol_node.get_merkle()
  }

  fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
    let mut node_id = self.root;
    let mut key_remaining = key;

    loop {
      let node = self.arena.node(node_id);

      match walk::<access_mode::Read, _>(
        key_remaining,
        &node.protocol_node.compressed_path_ref(),
        node,
      ) {
        WalkStop::Arrived => return node.protocol_node.value_as_slice(),
        WalkStop::PathDiverted { .. } | WalkStop::ChildNotFound { .. } => {
          return MptValue::None;
        }
        WalkStop::Descent {
          key_remaining: remaining,
          child_node,
          ..
        } => {
          node_id = child_node;
          key_remaining = remaining;
        }
      }
    }
  }

  fn proof(&self, key: &[u8]) -> TrieProof {
    if self.merkle_root() == MERKLE_NULL_NODE {
      return TrieProof::default();
    }

    let mut proof_nodes = Vec::new();
    let mut node_id = self.root;
    let mut key_remaining = key;
    let mut path_without_first_nibble = false;

    loop {
      let node = self.arena.node(node_id);
      let stop = walk::<access_mode::Read, _>(
        key_remaining,
        &node.protocol_node.compressed_path_ref(),
        node,
      );

      proof_nodes.push(node.proof_node(path_without_first_nibble));

      match stop {
        WalkStop::Arrived | WalkStop::PathDiverted { .. } | WalkStop::ChildNotFound { .. } => break,
        WalkStop::Descent {
          key_remaining: remaining,
          child_node,
          ..
        } => {
          path_without_first_nibble = CompressedPathRaw::has_second_nibble(
            node.protocol_node.compressed_path_ref().path_mask(),
          );
          node_id = child_node;
          key_remaining = remaining;
        }
      }
    }
    TrieProof::new(proof_nodes)
      .expect("nodes collected from a canonical trie path form a valid proof")
  }
}

/// Builds one canonical node from a non-empty ordered entry range and returns
/// both its arena identity and protocol Merkle hash.

fn build_node(
  arena: &mut NodeArena,
  entries: &[EntryRef<'_>],
  path_start: usize,
  is_root: bool,
) -> (NodeId, MerkleHash) {
  let path_end = if is_root {
    path_start
  } else {
    common_nibble_prefix_end(entries[0].0, entries[entries.len() - 1].0, path_start)
  };

  let compressed_path = if is_root {
    CompressedPathRaw::default()
  } else {
    compressed_path_from_key(entries[0].0, path_start..path_end)
  };

  let mut next_entry = 0;
  let node_value = if nibble_len(entries[0].0) == path_end {
    next_entry = 1;
    Some(match entries[0].1 {
      DeltaMptValue::Tombstone => Box::default(),
      DeltaMptValue::Present(value) => value.clone(),
    })
  } else {
    None
  };

  let mut child_merkles = [MERKLE_NULL_NODE; CHILDREN_COUNT];
  let mut child_links = Vec::new();

  while next_entry < entries.len() {
    let group_start = next_entry;
    let child_index = nibble_at(entries[group_start].0, path_end);

    next_entry += 1;
    while next_entry < entries.len() && nibble_at(entries[next_entry].0, path_end) == child_index {
      next_entry += 1;
    }

    let (node_id, merkle) = build_node(
      arena,
      &entries[group_start..next_entry],
      path_end + 1,
      false,
    );

    child_merkles[usize::from(child_index)] = merkle;
    child_links.push(ChildLink {
      child_index,
      node_id,
    });
  }

  let children = (!child_links.is_empty()).then_some(&child_merkles);
  let merkle = compute_merkle(
    compressed_path.as_ref(),
    !path_start.is_multiple_of(2),
    children,
    node_value.as_deref(),
  );

  let node = VanillaTrieNode::new(
    merkle,
    VanillaChildrenTable::from(child_merkles),
    node_value,
    compressed_path,
  );
  let node_id = arena.insert(StoredNode::new(node, child_links));

  (node_id, merkle)
}

#[cfg(test)]
mod tests {
  use super::*;
  use cfx_mpt::{CompressedPathTrait, TrieNodeTrait, VanillaChildrenTable, merkle::compute_merkle};

  fn masked_leaf(raw_path: u8) -> VanillaTrieNode<MerkleHash> {
    const VALUE: &[u8] = &[0xaa];

    let compressed_path =
      CompressedPathRaw::new(&[raw_path], CompressedPathRaw::first_nibble_mask());
    let merkle = compute_merkle(compressed_path.as_ref(), true, None, Some(VALUE));

    VanillaTrieNode::new(
      merkle,
      VanillaChildrenTable::default(),
      Some(VALUE.into()),
      compressed_path,
    )
  }

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
  fn arena_preserves_distinct_nodes_with_the_same_merkle_hash() {
    let first_leaf = masked_leaf(0x10);
    let second_leaf = masked_leaf(0x20);
    let mut arena = NodeArena::default();
    let first_leaf_id = arena.insert(StoredNode::new(first_leaf, Vec::new()));
    let second_leaf_id = arena.insert(StoredNode::new(second_leaf, Vec::new()));

    let first_leaf = &arena.node(first_leaf_id).protocol_node;
    let second_leaf = &arena.node(second_leaf_id).protocol_node;

    assert_eq!(first_leaf.get_merkle(), second_leaf.get_merkle());
    assert_eq!(first_leaf.compressed_path_ref().path_slice, &[0x10]);
    assert_eq!(second_leaf.compressed_path_ref().path_slice, &[0x20]);

    assert_ne!(first_leaf_id, second_leaf_id);
  }

  #[test]
  fn compressed_paths_preserve_nibble_boundaries() {
    let cases = [
      (0..4, &[0x12, 0x34][..], 0x00, 4),
      (1..4, &[0x12, 0x34][..], 0x0f, 3),
      (0..3, &[0x12, 0x30][..], 0xf0, 3),
      (1..3, &[0x12, 0x30][..], 0xff, 2),
      (1..1, &[0x10][..], 0xff, 0),
      (2..2, &[][..], 0x00, 0),
    ];

    for (range, expected_bytes, expected_mask, expected_steps) in cases {
      let path = compressed_path_from_key(&[0x12, 0x34], range);

      assert_eq!(path.path_slice(), expected_bytes);
      assert_eq!(path.path_mask(), expected_mask);
      assert_eq!(path.path_steps(), expected_steps);
    }
  }

  #[test]
  fn builder_matches_conflux_root_read_and_proof_fixtures() {
    let empty = CanonicalDeltaMpt::build(&DeltaMptEntries::default());
    assert_eq!(empty.merkle_root(), MERKLE_NULL_NODE);

    let mut entries = DeltaMptEntries::default();
    entries.set_tombstone(vec![0x12, 0x40]);
    entries.set_value(vec![0x12, 0x30], vec![0xaa].into_boxed_slice());

    let expected = "0x47d1332d15c79e86487b687395ef41e36cf860597d691e4628b7db37ddf6e6eb"
      .parse::<MerkleHash>()
      .expect("fixture root is valid");

    let mpt = CanonicalDeltaMpt::build(&entries);

    assert_eq!(mpt.merkle_root(), expected);
    assert_eq!(mpt.get(&[0x12, 0x30]), MptValue::Some(&[0xaa][..]));
    assert_eq!(mpt.get(&[0x12, 0x40]), MptValue::TombStone);
    assert_eq!(mpt.get(&[0x12, 0x50]), MptValue::None);
    assert_eq!(mpt.get(&[0x13, 0x30]), MptValue::None);

    let empty_proof = empty.proof(&[0x00]);
    assert!(empty_proof.is_valid_kv(&[0x00], None, &MERKLE_NULL_NODE,));

    let value_proof = mpt.proof(&[0x12, 0x30]);
    assert_eq!(value_proof.get_merkle_root(), &expected);
    assert!(value_proof.is_valid_kv(&[0x12, 0x30], Some(&[0xaa]), &expected,));

    let tombstone_proof = mpt.proof(&[0x12, 0x40]);
    assert!(tombstone_proof.is_valid_node_merkle(&[0x12, 0x40], &MptValue::TombStone, &expected,));

    let missing_proof = mpt.proof(&[0x12, 0x50]);
    assert!(missing_proof.is_valid_kv(&[0x12, 0x50], None, &expected,));
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
