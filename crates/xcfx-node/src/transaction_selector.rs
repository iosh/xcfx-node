use std::{
  cmp::Reverse,
  collections::{BTreeMap, BinaryHeap},
};

use cfx_executor::{spec::CommonParams, transaction_validation::PackingCheckResult};
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
  block_producer::RuntimeBlock,
  runtime_transaction::RuntimeTransaction,
  transaction_ingress::TransactionValidationContext,
  transaction_pool::{AccountKey, PoolEntryState, PoolSelectionInput, PoolViewEntry},
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

/// Priority key for the currently selectable transaction of one account.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AccountHeadCandidate {
  gas_price: U256,
  arrival_sequence: Reverse<u64>,
  hash: Reverse<H256>,
  account: Reverse<AccountKey>,
  entry_index: Reverse<usize>,
}

impl AccountHeadCandidate {
  fn new(account: AccountKey, entry_index: usize, entry: &PoolViewEntry) -> Self {
    Self {
      gas_price: *entry.transaction.gas_price(),
      arrival_sequence: Reverse(entry.arrival_sequence),
      hash: Reverse(entry.transaction.hash()),
      account: Reverse(account),
      entry_index: Reverse(entry_index),
    }
  }
}

fn pending_transactions_by_account(
  input: &PoolSelectionInput,
) -> BTreeMap<AccountKey, Vec<&PoolViewEntry>> {
  let mut transactions_by_account = BTreeMap::<AccountKey, Vec<&PoolViewEntry>>::new();

  for entry in &input.view.entries {
    let transaction = &entry.transaction;
    let state = input
      .entry_states
      .states
      .get(&transaction.hash())
      .copied()
      .expect("every pool view entry must have a selection state");

    if state != PoolEntryState::Pending {
      continue;
    }

    transactions_by_account
      .entry((transaction.sender(), transaction.space()))
      .or_default()
      .push(entry);
  }

  transactions_by_account
}

fn initial_account_heads(
  transactions_by_account: &BTreeMap<AccountKey, Vec<&PoolViewEntry>>,
) -> BinaryHeap<AccountHeadCandidate> {
  let mut candidates = BinaryHeap::new();

  for (&account, transactions) in transactions_by_account {
    let head = transactions
      .first()
      .expect("every pending account queue must have a head");

    candidates.push(AccountHeadCandidate::new(account, 0, head));
  }

  candidates
}

fn push_next_account_head(
  candidates: &mut BinaryHeap<AccountHeadCandidate>,
  transactions_by_account: &BTreeMap<AccountKey, Vec<&PoolViewEntry>>,
  selected: AccountHeadCandidate,
) {
  let account = selected.account.0;
  let next_index = selected
    .entry_index
    .0
    .checked_add(1)
    .expect("an account queue index must permit a successor");

  let transactions = transactions_by_account
    .get(&account)
    .expect("a selected account must have a pending queue");

  if let Some(next) = transactions.get(next_index) {
    candidates.push(AccountHeadCandidate::new(account, next_index, next));
  }
}

fn select_account_transactions<'a>(
  transactions_by_account: &BTreeMap<AccountKey, Vec<&'a PoolViewEntry>>,
  limits: TransactionSelectionLimits,
  mut accept: impl FnMut(&PoolViewEntry) -> bool,
) -> Vec<&'a PoolViewEntry> {
  let mut candidates = initial_account_heads(transactions_by_account);
  let mut selected = Vec::new();
  let mut selected_bytes = 0usize;

  while selected.len() < limits.max_transactions {
    let Some(candidate) = candidates.pop() else {
      break;
    };

    let account = candidate.account.0;
    let entry_index = candidate.entry_index.0;

    let entry = transactions_by_account
      .get(&account)
      .and_then(|transactions| transactions.get(entry_index))
      .copied()
      .expect("an account-head candidate must reference its queue");

    let transaction_size = entry.transaction.selection_size();

    let Some(next_selected_bytes) = selected_bytes.checked_add(transaction_size) else {
      continue;
    };

    if next_selected_bytes > limits.max_block_size_in_bytes {
      continue;
    }

    if !accept(entry) {
      continue;
    }

    selected_bytes = next_selected_bytes;
    selected.push(entry);

    push_next_account_head(&mut candidates, transactions_by_account, candidate);
  }

  selected
}

/// Transaction-related inputs consumed by the linear block producer.
pub(crate) struct BlockTransactionSelection {
  epoch_height: BlockHeight,
  block_gas_limit: U256,
  transactions: Vec<RuntimeTransaction>,
  base_price: SpaceMap<U256>,
}

impl BlockTransactionSelection {
  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self.epoch_height
  }

  pub(crate) fn block_gas_limit(&self) -> U256 {
    self.block_gas_limit
  }

  pub(crate) fn base_price(&self) -> &SpaceMap<U256> {
    &self.base_price
  }

  pub(crate) fn into_transactions(self) -> Vec<RuntimeTransaction> {
    self.transactions
  }
}

/// Selector output, including pool entries dropped only after a successful commit.
pub(crate) struct TransactionSelection {
  block_selection: BlockTransactionSelection,
  transactions_to_drop: Vec<RuntimeTransaction>,
}

impl TransactionSelection {
  pub(crate) fn into_block_selection_and_transactions_to_drop(
    self,
  ) -> (BlockTransactionSelection, Vec<RuntimeTransaction>) {
    (self.block_selection, self.transactions_to_drop)
  }
}

pub(crate) fn select_transactions(
  input: &PoolSelectionInput,
  parent: &RuntimeBlock,
  params: &CommonParams,
  validation: &TransactionValidationContext<'_>,
  block_gas_limit: U256,
  limits: TransactionSelectionLimits,
) -> TransactionSelection {
  let epoch_height = parent
    .header()
    .height()
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
      .header()
      .base_price()
      .expect("a post-CIP-1559 parent must contain base prices")
  };

  let min_base_price = params.min_base_price();
  let mut gas_used = SpaceMap::<U256>::default();
  let mut minimum_gas_price = SpaceMap::<Option<U256>>::default();

  let mut base_price = SpaceMap::zip4(gas_targets, gas_used, parent_base_price, min_base_price)
    .map_all(compute_next_price_tuple);

  let transactions_by_account = pending_transactions_by_account(input);
  let mut transactions_to_drop = Vec::new();

  let selected_entries = select_account_transactions(&transactions_by_account, limits, |entry| {
    let transaction = &entry.transaction;
    let space = transaction.space();

    match validation.check_runtime_for_packing(transaction) {
      PackingCheckResult::Pack => {}
      PackingCheckResult::Pending => return false,
      PackingCheckResult::Drop => {
        transactions_to_drop.push(entry.transaction.clone());
        return false;
      }
    }

    if space == Space::Ethereum && !can_pack_evm_transactions {
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

  let transactions = selected_entries
    .into_iter()
    .map(|entry| entry.transaction.clone())
    .collect();

  TransactionSelection {
    block_selection: BlockTransactionSelection {
      epoch_height,
      block_gas_limit,
      transactions,
      base_price,
    },
    transactions_to_drop,
  }
}
