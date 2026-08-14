//! Construction and access for in-memory Conflux current-delta MPT state.

use std::{collections::BTreeMap, ops::Range};

use cfx_mpt::{CompressedPathRaw, VanillaTrieNode};
use primitives::{MerkleHash, MptValue};

#[derive(Clone, Debug, Eq, PartialEq)]
enum CurrentDeltaValue {
  Tombstone,
  Present(Box<[u8]>),
}

/// Final physical entries used to construct a current-delta trie.
///
/// Missing keys are not stored. Every stored key maps to either a tombstone or
/// a present value, and iteration follows physical key order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CurrentDeltaEntries {
  entries: BTreeMap<Vec<u8>, CurrentDeltaValue>,
}

impl CurrentDeltaEntries {
  pub(crate) fn set_value(&mut self, key: Vec<u8>, value: Box<[u8]>) {
    self.entries.insert(key, CurrentDeltaValue::Present(value));
  }

  pub(crate) fn set_tombstone(&mut self, key: Vec<u8>) {
    self.entries.insert(key, CurrentDeltaValue::Tombstone);
  }

  pub(crate) fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
    match self.entries.get(key) {
      None => MptValue::None,
      Some(CurrentDeltaValue::Tombstone) => MptValue::TombStone,
      Some(CurrentDeltaValue::Present(value)) => MptValue::Some(value.as_ref()),
    }
  }

  pub(crate) fn iter(&self) -> impl Iterator<Item = (&[u8], MptValue<&[u8]>)> {
    self.entries.iter().map(|(key, value)| {
      let value = match value {
        CurrentDeltaValue::Tombstone => MptValue::TombStone,
        CurrentDeltaValue::Present(value) => MptValue::Some(value.as_ref()),
      };

      (key.as_slice(), value)
    })
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
    let mut entries = CurrentDeltaEntries::default();

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
    let mut entries = CurrentDeltaEntries::default();

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
}
