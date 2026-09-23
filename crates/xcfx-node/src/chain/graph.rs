//! Known local block topology and deterministic epoch execution plans.

use std::{
  collections::{BinaryHeap, HashMap, HashSet},
  sync::Arc,
};

use cfx_parameters::consensus_internal::EPOCH_EXECUTED_BLOCK_BOUND;
use cfx_types::H256;
use imbl::OrdMap;
use thiserror::Error;

use crate::block_producer::{BlockParent, RuntimeBlock};

#[derive(Debug, Error)]
pub(crate) enum GraphError {
  #[error("local block retention limit reached ({0}); prune detached blocks or reset the node")]
  Capacity(usize),
  #[error("unknown local block {0:?}")]
  UnknownBlock(H256),
  #[error("block does not extend its parent at the next height")]
  InvalidHeight,
  #[error("a block cannot repeat its parent or a referee in its reference list")]
  DuplicateReference,
  #[error("local block production requires an unchanged PoS reference")]
  UnsupportedPosReference,
  #[error("local block production supports only blame-zero headers")]
  UnsupportedBlame,
  #[error("the pivot parent must already belong to the execution history")]
  MissingExecutionParent,
}

/// A fixed origin and immutable blocks; choosing a pivot is a separate operation.
#[derive(Clone)]
pub(crate) struct BlockGraph {
  origin: BlockParent,
  blocks: OrdMap<H256, Arc<RuntimeBlock>>,
  max_blocks: usize,
}

pub(crate) struct OrderedEpoch {
  pub(crate) blocks: Vec<RuntimeBlock>,
  pub(crate) skipped: Vec<RuntimeBlock>,
}

impl BlockGraph {
  pub(crate) fn new(origin: BlockParent, max_blocks: usize) -> Self {
    Self {
      origin,
      blocks: OrdMap::new(),
      max_blocks,
    }
  }

  pub(crate) fn len(&self) -> usize {
    self.blocks.len()
  }

  pub(crate) fn check_capacity(&self) -> Result<(), GraphError> {
    if self.len() >= self.max_blocks {
      return Err(GraphError::Capacity(self.max_blocks));
    }
    Ok(())
  }

  pub(crate) fn block(&self, hash: &H256) -> Option<&Arc<RuntimeBlock>> {
    self.blocks.get(hash)
  }

  pub(crate) fn parent_input(&self, hash: H256) -> Result<BlockParent, GraphError> {
    if hash == self.origin.hash {
      return Ok(self.origin);
    }
    self
      .blocks
      .get(&hash)
      .map(|block| BlockParent::from_header(block.header()))
      .ok_or(GraphError::UnknownBlock(hash))
  }

  /// All predecessor blocks must be known before a new block is registered.
  /// This insertion order makes cycles impossible without a second graph pass.
  pub(crate) fn insert(&mut self, block: RuntimeBlock) -> Result<bool, GraphError> {
    let hash = block.hash();
    if hash == self.origin.hash || self.blocks.contains_key(&hash) {
      return Ok(false);
    }
    self.check_capacity()?;
    let header = block.header();
    let parent = self.parent_input(*header.parent_hash())?;
    if parent.height.checked_add(1) != Some(header.height()) {
      return Err(GraphError::InvalidHeight);
    }
    if *header.pos_reference() != Some(self.origin.pos_reference) {
      return Err(GraphError::UnsupportedPosReference);
    }
    if header.blame() != 0 {
      return Err(GraphError::UnsupportedBlame);
    }
    let mut predecessors = HashSet::from([parent.hash]);
    for referee in header.referee_hashes() {
      self.parent_input(*referee)?;
      if !predecessors.insert(*referee) {
        return Err(GraphError::DuplicateReference);
      }
    }
    self.blocks.insert(hash, Arc::new(block));
    Ok(true)
  }

  /// Returns the selected pivot's parent path after the fixed origin.
  pub(crate) fn pivot_path(&self, pivot: H256) -> Result<Vec<H256>, GraphError> {
    let mut path = Vec::new();
    let mut cursor = pivot;
    while cursor != self.origin.hash {
      let block = self
        .blocks
        .get(&cursor)
        .ok_or(GraphError::UnknownBlock(cursor))?;
      path.push(cursor);
      cursor = *block.header().parent_hash();
    }
    path.reverse();
    Ok(path)
  }

  /// Computes the execution order for the selected pivot's epoch.
  ///
  /// `past` must include all blocks assigned to preceding epochs, including
  /// skipped blocks and the pivot parent. Only the pivot's referee past can
  /// introduce additional members.
  pub(crate) fn ordered_epoch(
    &self,
    pivot: H256,
    past: &HashSet<H256>,
  ) -> Result<OrderedEpoch, GraphError> {
    let pivot_block = self
      .blocks
      .get(&pivot)
      .ok_or(GraphError::UnknownBlock(pivot))?;
    if !past.contains(pivot_block.header().parent_hash()) {
      return Err(GraphError::MissingExecutionParent);
    }
    let mut pending = pivot_block.header().referee_hashes().clone();
    let mut members = HashSet::new();
    while let Some(hash) = pending.pop() {
      if hash == self.origin.hash || past.contains(&hash) || !members.insert(hash) {
        continue;
      }
      let block = &self.blocks[&hash];
      pending.push(*block.header().parent_hash());
      pending.extend(block.header().referee_hashes());
    }

    // Count references from remaining epoch members. Zero counts identify the
    // sinks removed by the reverse topological traversal.
    let mut successors: HashMap<H256, usize> = members.iter().map(|hash| (*hash, 0)).collect();
    for hash in &members {
      for predecessor in self.predecessors(hash) {
        if let Some(count) = successors.get_mut(&predecessor) {
          *count += 1;
        }
      }
    }
    // Conflux chooses the largest hash among available sinks, then reverses the
    // result. A forward smallest-hash traversal is not equivalent.
    let mut ready: BinaryHeap<H256> = successors
      .iter()
      .filter_map(|(hash, count)| (*count == 0).then_some(*hash))
      .collect();
    let mut order = Vec::with_capacity(members.len() + 1);
    while let Some(hash) = ready.pop() {
      order.push(hash);
      for predecessor in self.predecessors(&hash) {
        if let Some(count) = successors.get_mut(&predecessor) {
          *count -= 1;
          if *count == 0 {
            ready.push(predecessor);
          }
        }
      }
    }
    order.reverse();
    order.push(pivot);
    // Only the suffix ending at the pivot is executed. The skipped prefix still
    // belongs to this epoch and must be included in subsequent `past` sets.
    let cut = order.len().saturating_sub(EPOCH_EXECUTED_BLOCK_BOUND);
    let blocks = order.split_off(cut);
    Ok(OrderedEpoch {
      blocks: blocks
        .into_iter()
        .map(|hash| self.blocks[&hash].as_ref().clone())
        .collect(),
      skipped: order
        .into_iter()
        .map(|hash| self.blocks[&hash].as_ref().clone())
        .collect(),
    })
  }

  fn predecessors(&self, hash: &H256) -> impl Iterator<Item = H256> + '_ {
    let header = self.blocks[hash].header();
    std::iter::once(*header.parent_hash()).chain(header.referee_hashes().iter().copied())
  }

  pub(crate) fn execution_block_count(
    &self,
    referees: &[H256],
    past: &HashSet<H256>,
  ) -> Result<usize, GraphError> {
    let mut seen = HashSet::new();
    let mut pending = referees.to_vec();
    while let Some(hash) = pending.pop() {
      if hash == self.origin.hash || past.contains(&hash) || !seen.insert(hash) {
        continue;
      }
      let block = self
        .blocks
        .get(&hash)
        .ok_or(GraphError::UnknownBlock(hash))?;
      pending.push(*block.header().parent_hash());
      pending.extend(block.header().referee_hashes());
    }
    Ok((seen.len() + 1).min(EPOCH_EXECUTED_BLOCK_BOUND))
  }
}
