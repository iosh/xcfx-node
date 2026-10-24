use std::{
  cmp::{Ordering, Reverse},
  collections::BinaryHeap,
};

use cfx_executor::spec::CommonParams;
use cfx_parameters::{
  block::{
    MAX_BLOCK_SIZE_IN_BYTES, MAX_TRANSACTION_COUNT_PER_BLOCK, cspace_block_gas_limit_after_cip1559,
    espace_block_gas_limit,
  },
  consensus_internal::ELASTICITY_MULTIPLIER,
};
use cfx_types::{H256, Space, SpaceMap, U256};
use primitives::{
  block::BlockHeight,
  block_header::{compute_next_price, compute_next_price_tuple},
};

use crate::{
  block_producer::BlockParent, runtime_transaction::RuntimeTransaction,
  transaction_pool::PoolTransaction,
};

/// Resource limits applied by the deterministic selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TransactionSelectionLimits {
  max_transactions: usize,
  max_block_size_in_bytes: usize,
}

impl Default for TransactionSelectionLimits {
  fn default() -> Self {
    Self {
      max_transactions: MAX_TRANSACTION_COUNT_PER_BLOCK,
      max_block_size_in_bytes: MAX_BLOCK_SIZE_IN_BYTES,
    }
  }
}

/// One account's head and the remaining borrowed nonce prefix.
struct AccountHeadCandidate<'a> {
  priority: (U256, Reverse<u64>, Reverse<H256>),
  pooled: &'a PoolTransaction,
  remaining: &'a [&'a PoolTransaction],
}

impl<'a> AccountHeadCandidate<'a> {
  fn new(pooled: &'a PoolTransaction, remaining: &'a [&'a PoolTransaction]) -> Self {
    Self {
      priority: (
        *pooled.transaction().gas_price(),
        Reverse(pooled.arrival_sequence()),
        Reverse(pooled.transaction().hash()),
      ),
      pooled,
      remaining,
    }
  }
}

impl PartialEq for AccountHeadCandidate<'_> {
  fn eq(&self, other: &Self) -> bool {
    self.priority == other.priority
  }
}

impl Eq for AccountHeadCandidate<'_> {}

impl PartialOrd for AccountHeadCandidate<'_> {
  fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
    Some(self.cmp(other))
  }
}

impl Ord for AccountHeadCandidate<'_> {
  fn cmp(&self, other: &Self) -> Ordering {
    // Pool hashes are unique, so the remaining queue cannot break a priority tie.
    self.priority.cmp(&other.priority)
  }
}

fn initial_account_heads<'a>(
  mut transactions: &'a [&'a PoolTransaction],
) -> BinaryHeap<AccountHeadCandidate<'a>> {
  let mut candidates = BinaryHeap::new();
  while let Some((head, tail)) = transactions.split_first() {
    let address = head.transaction().sender_with_space();
    let next_account = tail
      .iter()
      .position(|pooled| pooled.transaction().sender_with_space() != address)
      .unwrap_or(tail.len());
    let (remaining, next) = tail.split_at(next_account);
    candidates.push(AccountHeadCandidate::new(head, remaining));
    transactions = next;
  }
  candidates
}

fn select_account_transactions<'a>(
  transactions: &'a [&'a PoolTransaction],
  limits: TransactionSelectionLimits,
  mut accept: impl FnMut(&PoolTransaction) -> bool,
) -> Vec<&'a PoolTransaction> {
  let mut candidates = initial_account_heads(transactions);
  let mut selected = Vec::new();
  let mut selected_bytes = 0usize;

  while selected.len() < limits.max_transactions {
    let Some(candidate) = candidates.pop() else {
      break;
    };

    let pooled = candidate.pooled;

    let transaction_size = pooled.transaction().selection_size();

    let Some(next_selected_bytes) = selected_bytes.checked_add(transaction_size) else {
      continue;
    };

    if next_selected_bytes > limits.max_block_size_in_bytes {
      continue;
    }

    if !accept(pooled) {
      continue;
    }

    selected_bytes = next_selected_bytes;
    selected.push(pooled);

    if let Some((next, remaining)) = candidate.remaining.split_first() {
      candidates.push(AccountHeadCandidate::new(next, remaining));
    }
  }

  selected
}

/// Transaction-related inputs consumed by the linear block producer.
pub(crate) struct BlockTransactionSelection {
  epoch_height: BlockHeight,
  transactions: Vec<RuntimeTransaction>,
  base_price: SpaceMap<U256>,
}

impl BlockTransactionSelection {
  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self.epoch_height
  }

  pub(crate) fn base_price(&self) -> &SpaceMap<U256> {
    &self.base_price
  }

  pub(crate) fn into_transactions(self) -> Vec<RuntimeTransaction> {
    self.transactions
  }
}

/// Block transactions and whether another eSpace packing height is needed.
pub(crate) struct TransactionSelection {
  block_selection: BlockTransactionSelection,
  waiting_for_espace: bool,
}

impl TransactionSelection {
  /// No transactions selected for execution.
  pub(crate) fn is_empty(&self) -> bool {
    self.block_selection.transactions.is_empty()
  }

  /// An eSpace account head fits the packing constraints but needs an eSpace
  /// block height. It has not been included in the block selection.
  pub(crate) fn is_waiting_for_espace(&self) -> bool {
    self.waiting_for_espace
  }

  pub(crate) fn into_block_selection(self) -> BlockTransactionSelection {
    self.block_selection
  }
}

/// Selects from protocol-valid, affordable nonce prefixes for each account and Space.
/// Runtime supplies contiguous account groups in ascending nonce order under one
/// state version; packing height, prices, and block budgets are checked here.
pub(crate) fn select_transactions(
  candidates: Vec<&PoolTransaction>,
  parent: &BlockParent,
  params: &CommonParams,
  block_gas_limit: U256,
  limits: TransactionSelectionLimits,
) -> TransactionSelection {
  let epoch_height = parent
    .height
    .checked_add(1)
    .expect("a parent block must permit a subsequent height");

  let cip1559_height = params.transition_heights.cip1559;
  let can_pack_evm_transactions = params.can_pack_evm_transaction(epoch_height);

  let gas_limits = SpaceMap::new(
    cspace_block_gas_limit_after_cip1559(block_gas_limit),
    espace_block_gas_limit(can_pack_evm_transactions, block_gas_limit),
  );

  let gas_targets = gas_limits.map_all(|gas_limit| gas_limit / ELASTICITY_MULTIPLIER);

  let parent_base_price = if epoch_height == cip1559_height {
    params.init_base_price()
  } else {
    parent
      .base_price
      .expect("a post-CIP-1559 parent must contain base prices")
  };

  let min_base_price = params.min_base_price();
  let mut gas_used = SpaceMap::<U256>::default();
  let mut minimum_gas_price = SpaceMap::<Option<U256>>::default();

  let mut base_price = SpaceMap::zip4(gas_targets, gas_used, parent_base_price, min_base_price)
    .map_all(compute_next_price_tuple);

  let mut waiting_for_espace = false;

  let selected = select_account_transactions(&candidates, limits, |pooled| {
    let transaction = pooled.transaction();
    let space = transaction.space();

    if space == Space::Ethereum && !can_pack_evm_transactions {
      // Size, nonce and protocol checks have already passed. Check whether this
      // head fits an empty eSpace block, without admitting it at this height.
      let espace_gas_limit = espace_block_gas_limit(true, block_gas_limit);
      if !waiting_for_espace && *transaction.gas() <= espace_gas_limit {
        let required_price = compute_next_price(
          espace_gas_limit / ELASTICITY_MULTIPLIER,
          *transaction.gas(),
          parent_base_price[Space::Ethereum],
          min_base_price[Space::Ethereum],
        );
        waiting_for_espace = *transaction.gas_price() >= required_price;
      }
      return false;
    }

    let transaction_gas = *transaction.gas();

    let Some(next_space_gas) = gas_used[space].checked_add(transaction_gas) else {
      return false;
    };

    if next_space_gas > gas_limits[space] {
      return false;
    }

    let next_minimum_gas_price = match minimum_gas_price[space] {
      Some(current) => current.min(*transaction.gas_price()),
      None => *transaction.gas_price(),
    };

    let next_base_price = compute_next_price(
      gas_targets[space],
      next_space_gas,
      parent_base_price[space],
      min_base_price[space],
    );

    if next_minimum_gas_price < next_base_price {
      return false;
    }

    gas_used[space] = next_space_gas;
    minimum_gas_price[space] = Some(next_minimum_gas_price);
    base_price[space] = next_base_price;

    true
  });

  let transactions = selected
    .into_iter()
    .map(|pooled| pooled.transaction().clone())
    .collect();

  TransactionSelection {
    block_selection: BlockTransactionSelection {
      epoch_height,
      transactions,
      base_price,
    },
    waiting_for_espace,
  }
}
