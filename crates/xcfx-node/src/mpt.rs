//! In-memory Conflux MPT construction and access.

use std::ops::Range;

use cfx_mpt::{
  CompressedPathRaw, CompressedPathTrait, TrieNodeTrait, TrieProof, TrieProofNode,
  VanillaChildrenTable, VanillaTrieNode,
  children_table::CHILDREN_COUNT,
  merkle::compute_merkle,
  walk::{GetChildTrait, WalkStop, walk},
};
use cfx_storage_types::access_mode;
use primitives::{MERKLE_NULL_NODE, MerkleHash, MptValue};

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

/// One borrowed key-value entry consumed by MPT construction.
pub(crate) struct MptEntry<'a> {
  pub(crate) key: &'a [u8],
  pub(crate) value: &'a [u8],
}

/// In-memory MPT built from one complete ordered entry set.
pub(crate) struct Mpt {
  arena: NodeArena,
  root: NodeId,
}

impl Mpt {
  pub(crate) fn build<'a>(entries: impl IntoIterator<Item = MptEntry<'a>>) -> Self {
    let ordered_entries = entries.into_iter().collect::<Vec<_>>();

    let mut arena = NodeArena::default();
    let root = if ordered_entries.is_empty() {
      arena.insert(StoredNode::new(VanillaTrieNode::default(), Vec::new()))
    } else {
      build_node(&mut arena, &ordered_entries, 0, true).0
    };

    Self { arena, root }
  }

  pub(crate) fn merkle_root(&self) -> MerkleHash {
    *self.arena.node(self.root).protocol_node.get_merkle()
  }

  pub(crate) fn get(&self, key: &[u8]) -> MptValue<&[u8]> {
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

  pub(crate) fn proof(&self, key: &[u8]) -> TrieProof {
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
    TrieProof::new(proof_nodes).expect("nodes collected from an MPT path form a valid proof")
  }
}

fn index_key_byte_len(value_count: usize) -> usize {
  let mut max_index = match value_count {
    0 => return 0,
    1 => return 1,
    _ => value_count - 1,
  };

  let mut byte_len = 0;

  while max_index != 0 {
    byte_len += 1;
    max_index >>= 8;
  }

  byte_len
}

/// Computes the Conflux MPT commitment for a complete ordered list.
///
/// Values are keyed by their zero-based positions using the minimum
/// fixed-width big-endian encoding required for the whole list.
pub(crate) fn indexed_mpt_root<'a>(values: impl ExactSizeIterator<Item = &'a [u8]>) -> MerkleHash {
  let value_count = values.len();

  if value_count == 0 {
    return MERKLE_NULL_NODE;
  }

  let key_byte_len = index_key_byte_len(value_count);
  let mut key_bytes = Vec::with_capacity(value_count * key_byte_len);

  for index in 0..value_count {
    let big_endian_bytes = index.to_be_bytes();
    let key_start = big_endian_bytes.len() - key_byte_len;
    key_bytes.extend_from_slice(&big_endian_bytes[key_start..]);
  }

  let mpt = Mpt::build(
    key_bytes
      .chunks_exact(key_byte_len)
      .zip(values)
      .map(|(key, value)| MptEntry { key, value }),
  );

  let root = mpt.arena.node(mpt.root);

  // Conflux list commitments omit an explicit root whose only child is zero.
  if let [only_child] = root.child_links.as_ref() {
    assert_eq!(
      only_child.child_index, 0,
      "an indexed MPT's only root child must have index zero",
    );

    return *mpt
      .arena
      .node(only_child.node_id)
      .protocol_node
      .get_merkle();
  }

  mpt.merkle_root()
}

/// Builds one node from a non-empty ordered entry range and returns
/// both its arena identity and protocol Merkle hash.
fn build_node(
  arena: &mut NodeArena,
  entries: &[MptEntry<'_>],
  path_start: usize,
  is_root: bool,
) -> (NodeId, MerkleHash) {
  let path_end = if is_root {
    path_start
  } else {
    common_nibble_prefix_end(entries[0].key, entries[entries.len() - 1].key, path_start)
  };

  let compressed_path = if is_root {
    CompressedPathRaw::default()
  } else {
    compressed_path_from_key(entries[0].key, path_start..path_end)
  };

  let mut next_entry = 0;
  let node_value: Option<Box<[u8]>> = if nibble_len(entries[0].key) == path_end {
    next_entry = 1;
    Some(entries[0].value.into())
  } else {
    None
  };

  let mut child_merkles = [MERKLE_NULL_NODE; CHILDREN_COUNT];
  let mut child_links = Vec::new();

  while next_entry < entries.len() {
    let group_start = next_entry;
    let child_index = nibble_at(entries[group_start].key, path_end);

    next_entry += 1;
    while next_entry < entries.len() && nibble_at(entries[next_entry].key, path_end) == child_index
    {
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

  use hex_literal::hex;

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
    let empty = Mpt::build(std::iter::empty::<MptEntry<'static>>());
    assert_eq!(empty.merkle_root(), MERKLE_NULL_NODE);

    let value_key = [0x12, 0x30];
    let value = [0xaa];
    let tombstone_key = [0x12, 0x40];
    let tombstone: &[u8] = &[];

    let expected = MerkleHash::from(hex!(
      "47d1332d15c79e86487b687395ef41e36cf860597d691e4628b7db37ddf6e6eb"
    ));

    let mpt = Mpt::build([
      MptEntry {
        key: &value_key,
        value: &value,
      },
      MptEntry {
        key: &tombstone_key,
        value: tombstone,
      },
    ]);

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
  fn indexed_mpt_root_matches_protocol_boundaries() {
    assert_eq!(
      indexed_mpt_root(std::iter::empty::<&[u8]>()),
      MERKLE_NULL_NODE,
    );
    assert_eq!(index_key_byte_len(256), 1);
    assert_eq!(index_key_byte_len(257), 2);

    // Conflux general_2 fixture containing 17 transactions.
    let transaction_hashes = hex!(
      "7767986b6835cfc530cab984b89d331d9ab0b0758ee7287f8b8b6f52f0c7470b"
      "3e2846dc2c2795a43716b46c5ea38e597ea4ee095b2b4adc7e0b51fc1ff1d5ce"
      "fb8cffa9232edbb1130e80e29675d7da8dc4993a1b4c2503c02d67746eabefe3"
      "3fed13ee913d83344c1b178dbf7531461919e3e2a4db1cd8416322e7eecc3d4f"
      "c3fcb49723e58b78f3f842ac0c7eb80b11c2eb3563dbb46b271d6ef1dc6e9179"
      "d48711163ae50de225dbd9f5fdf61f9b85d85b100b2560f01432127b7201c0df"
      "fad51b86c72bec2208086a059bb99d07599e3ec76c984933fabdf329fa2e8ff6"
      "3f89b1e16bf54640c341a81cc6703eac6ec34899876ddb9953c7f4760a07796b"
      "d98e01f94c0c51829508b7e4c7f1a3bd01c9301a5e1128f15b25cdaa26d70eae"
      "6e883e1ca7b0d4fc1af64e9a3a54cabb4396cb7849675c92f4b5250635764daf"
      "abf1fc8f37bb8ab9ae430a7d5b905c5ec3acb9ce8d49d73755a8b39553b19b3d"
      "f488b6681a8ddb8e3656f8310b4ca4798a212747e6dbdb306de516752bff8e35"
      "c5476cd30d1a9ce9ceee328605ef2ee05e54a2fbe42305aee6aeb46ebffed849"
      "32d224dd711caa94631f53c7c3177408ecb0809c6bb4ab5b6cee81dcc4792d51"
      "83e991d743a755020e798c343c61467528e29a9f1b5e21c2902cac550ac5f392"
      "35931bf46108b6ba81a88ba7290eb9bb2d5f7b3c5d3acca718abc8e5852edbb0"
      "76eca5387d51b01f4a0a1d5b9f8fbd2b0eb5b6531b24814b684e34e19cf727ee"
    );

    assert_eq!(
      indexed_mpt_root(transaction_hashes.chunks_exact(32)),
      MerkleHash::from(hex!(
        "afe85264b77b1f814769930ed608a8d47fdd2f8e42496f8284d3ebf6582bccd8"
      )),
    );
  }

  #[test]
  fn indexed_mpt_root_matches_genesis_commitments() {
    // Conflux general_2 Genesis transaction hashes.
    let transaction_hashes = hex!(
      "a73d49355986057ca810cb256da2c0a4207a0ad35dcc52bda0fc8748771d369b"
      "11f844dfca244d2b98b626d730c2d62cfe01dd04dd51b3db2448be7bb18b7ed3"
      "dfd085f9771f9497a003a95be530e11cc596e131d1ec9256992ccf9995363015"
      "31d27116ac2721bee930e607a72a8874929eea60d3f7ab28ce809e6c169f8a3a"
      "d1162c03ee724ce6d3fb2165082fb2afbc48b9063ec03333efc1ffd74e320ac9"
      "8e15cd6668803852ed817dc78eabcdef01a6cdbd03549e8c4856ae77510ce258"
      "803b6fec1cfc3532b89c4d6d1476cbd988fcb5487be0e52230aaa8a2fe7bd10b"
      "488133658a149f00eef17650c16c91aaf98cd15ccb17be201839b727c306dbea"
    );

    assert_eq!(
      indexed_mpt_root(transaction_hashes.chunks_exact(32)),
      MerkleHash::from(hex!(
        "8208dfdbb409f7a3e41386a8eaaa6412ad4df158fc04a09b499c1a004b53d469"
      )),
    );

    let empty_block_receipt_root = indexed_mpt_root(std::iter::empty::<&[u8]>());
    let empty_block_receipt_root_bytes: &[u8] = empty_block_receipt_root.as_bytes();

    assert_eq!(
      indexed_mpt_root(std::iter::once(empty_block_receipt_root_bytes)),
      MerkleHash::from(hex!(
        "09f8709ea9f344a810811a373b30861568f5686e649d6177fd92ea2db7477508"
      )),
    );
  }
}
