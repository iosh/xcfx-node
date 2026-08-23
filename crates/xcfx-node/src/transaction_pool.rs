use std::{
  collections::{BTreeMap, HashMap},
  sync::Arc,
};

use cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT;
use cfx_types::{Address, H256, Space, U128, U256, U512};
use primitives::{Account, SignedTransaction, transaction::TransactionError};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TransactionKey {
  sender: Address,
  space: Space,
  nonce: U256,
}

impl TransactionKey {
  pub(crate) fn from_transaction(transaction: &SignedTransaction) -> Self {
    Self {
      sender: transaction.sender,
      space: transaction.space(),
      nonce: *transaction.nonce(),
    }
  }
}

pub(crate) type AccountKey = (Address, Space);

pub(crate) struct PoolAccountState {
  pub(crate) committed_nonce: U256,
  pub(crate) balance: U256,
}

impl PoolAccountState {
  pub(crate) fn from_account(account: &Account) -> Self {
    Self {
      committed_nonce: account.nonce,
      balance: account.balance,
    }
  }
}

pub(crate) fn pool_transaction_cost(
  transaction: &SignedTransaction,
  sponsored_gas: U256,
  sponsored_storage: u64,
) -> U256 {
  let gas = *transaction.gas() - sponsored_gas;

  let gas_cost = if gas.full_mul(*transaction.gas_price()) > U512::from(U128::max_value()) {
    U256::from(U128::max_value())
  } else {
    gas * *transaction.gas_price()
  };

  let storage_cost = transaction
    .storage_limit()
    .map_or_else(U256::zero, |limit| {
      let unsponsored_storage = limit
        .checked_sub(sponsored_storage)
        .expect("sponsored storage cannot exceed transaction storage limit");

      U256::from(unsponsored_storage) * *DRIPS_PER_STORAGE_COLLATERAL_UNIT
    });

  let value_cap = U256::from(u64::MAX) * U256::from(U128::max_value());
  let value = if *transaction.value() > value_cap {
    value_cap
  } else {
    *transaction.value()
  };

  value + gas_cost + storage_cost
}

pub(crate) struct PoolReadinessInputs {
  pub(crate) account_states: BTreeMap<AccountKey, PoolAccountState>,
  pub(crate) transaction_costs: BTreeMap<H256, U256>,
}

impl PoolReadinessInputs {
  pub(crate) fn new(
    account_states: BTreeMap<AccountKey, PoolAccountState>,
    transaction_costs: BTreeMap<H256, U256>,
  ) -> Self {
    Self {
      account_states,
      transaction_costs,
    }
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PoolEntryState {
  Pending,
  QueuedNonceGap {
    expected_nonce: U256,
    actual_nonce: U256,
  },
  QueuedInsufficientBalance {
    required: U256,
    available: U256,
  },
  Stale,
}

pub(crate) struct PoolEntryStates {
  pub(crate) states: BTreeMap<H256, PoolEntryState>,
}

#[derive(Clone, Copy)]
enum BlockedState {
  NonceGap { expected_nonce: U256 },
  InsufficientBalance { required: U256, available: U256 },
}

pub(crate) struct PoolEntry {
  pub(crate) transaction: Arc<SignedTransaction>,
  pub(crate) arrival_sequence: u64,
}

pub(crate) struct TransactionPoolPolicy {
  pub(crate) max_transactions: usize,
}

impl TransactionPoolPolicy {
  pub(crate) const fn new(max_transactions: usize) -> Self {
    Self { max_transactions }
  }
}

pub(crate) enum TransactionPoolInsertOutcome {
  Inserted,
  Replaced { previous: Arc<SignedTransaction> },
}

pub(crate) struct PoolViewEntry {
  pub(crate) transaction: Arc<SignedTransaction>,
  pub(crate) arrival_sequence: u64,
}

pub(crate) struct TransactionPoolView {
  pub(crate) view_id: H256,
  pub(crate) pool_revision: u64,
  pub(crate) entries: Vec<PoolViewEntry>,
}

pub(crate) struct PoolSelectionInput {
  pub(crate) view: TransactionPoolView,
  pub(crate) entry_states: PoolEntryStates,
}

impl TransactionPoolView {
  pub(crate) fn is_current(&self, view_id: H256, pool_revision: u64) -> bool {
    self.view_id == view_id && self.pool_revision == pool_revision
  }
}

pub(crate) struct TransactionPool {
  by_key: BTreeMap<TransactionKey, PoolEntry>,
  key_by_hash: HashMap<H256, TransactionKey>,
  revision: u64,
  next_arrival_sequence: u64,
}

impl TransactionPool {
  pub(crate) fn new() -> Self {
    Self {
      by_key: BTreeMap::new(),
      key_by_hash: HashMap::new(),
      revision: 0,
      next_arrival_sequence: 0,
    }
  }
  pub(crate) fn revision(&self) -> u64 {
    self.revision
  }

  pub(crate) fn len(&self) -> usize {
    self.by_key.len()
  }

  pub(crate) fn get_by_hash(&self, hash: H256) -> Option<&PoolEntry> {
    self
      .key_by_hash
      .get(&hash)
      .and_then(|key| self.by_key.get(key))
  }

  pub(crate) fn get_by_key(&self, key: &TransactionKey) -> Option<&PoolEntry> {
    self.by_key.get(key)
  }

  pub(crate) fn insert(
    &mut self,
    transaction: Arc<SignedTransaction>,
    policy: &TransactionPoolPolicy,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    let hash = transaction.hash();

    if self.key_by_hash.contains_key(&hash) {
      return Err(TransactionError::AlreadyImported);
    }
    let key = TransactionKey::from_transaction(&transaction);
    if let Some((previous, previous_hash)) = self
      .by_key
      .get(&key)
      .map(|entry| (Arc::clone(&entry.transaction), entry.transaction.hash()))
    {
      if !Self::replacement_is_sufficient(&transaction, &previous) {
        return Err(TransactionError::TooCheapToReplace);
      }

      let arrival_sequence = self.next_arrival_sequence();
      self
        .key_by_hash
        .remove(&previous_hash)
        .expect("pool hash index must match primary index");

      let replaced = self.by_key.insert(
        key,
        PoolEntry {
          transaction,
          arrival_sequence,
        },
      );

      let replaced = replaced.expect("pool key must exist during replacement");
      assert_eq!(
        replaced.transaction.hash(),
        previous_hash,
        "pool primary index must contain the replaced transaction",
      );

      assert!(
        self.key_by_hash.insert(hash, key).is_none(),
        "pool hash index must not contain a replacement hash",
      );
      self.bump_revision();

      return Ok(TransactionPoolInsertOutcome::Replaced { previous });
    }

    if self.by_key.len() >= policy.max_transactions {
      return Err(TransactionError::LimitReached);
    }

    let arrival_sequence = self.next_arrival_sequence();
    assert!(
      self.key_by_hash.insert(hash, key).is_none(),
      "pool hash index must not contain a new transaction hash",
    );

    self.by_key.insert(
      key,
      PoolEntry {
        transaction,
        arrival_sequence,
      },
    );
    self.bump_revision();

    Ok(TransactionPoolInsertOutcome::Inserted)
  }

  fn next_arrival_sequence(&mut self) -> u64 {
    let sequence = self.next_arrival_sequence;
    self.next_arrival_sequence = sequence
      .checked_add(1)
      .expect("transaction pool arrival sequence exhausted");
    sequence
  }

  fn bump_revision(&mut self) {
    self.revision = self
      .revision
      .checked_add(1)
      .expect("transaction pool revision exhausted");
  }

  fn replacement_is_sufficient(
    replacement: &SignedTransaction,
    previous: &SignedTransaction,
  ) -> bool {
    let previous_price = *previous.gas_price();
    let required_price = if previous_price < U256::from(100) {
      previous_price + U256::one()
    } else {
      previous_price + (previous_price / U256::from(100)) * U256::from(2)
    };

    *replacement.gas_price() >= required_price
  }

  pub(crate) fn remove_by_hash(&mut self, hash: H256) -> Option<Arc<SignedTransaction>> {
    let transaction = self.remove_by_hash_without_revision(hash);
    if transaction.is_some() {
      self.bump_revision();
    }
    transaction
  }

  fn remove_by_hash_without_revision(&mut self, hash: H256) -> Option<Arc<SignedTransaction>> {
    let key = self.key_by_hash.remove(&hash)?;
    let entry = self
      .by_key
      .remove(&key)
      .expect("pool hash index must match primary index");
    assert_eq!(
      entry.transaction.hash(),
      hash,
      "pool primary index must contain the indexed transaction",
    );
    Some(entry.transaction)
  }

  pub(crate) fn remove_stale(
    &mut self,
    entry_states: &PoolEntryStates,
  ) -> Vec<Arc<SignedTransaction>> {
    let stale_hashes = entry_states
      .states
      .iter()
      .filter_map(|(hash, state)| matches!(*state, PoolEntryState::Stale).then_some(*hash))
      .collect::<Vec<_>>();

    let removed = stale_hashes
      .into_iter()
      .filter_map(|hash| self.remove_by_hash_without_revision(hash))
      .collect::<Vec<_>>();
    if !removed.is_empty() {
      self.bump_revision();
    }
    removed
  }

  pub(crate) fn derive_entry_states(&self, inputs: &PoolReadinessInputs) -> PoolEntryStates {
    let mut next_nonce = BTreeMap::<AccountKey, U256>::new();
    let mut remaining_balance = BTreeMap::<AccountKey, U256>::new();
    let mut blocked_state = BTreeMap::<AccountKey, Option<BlockedState>>::new();
    let mut states = BTreeMap::new();

    for (key, entry) in &self.by_key {
      let account_key = (key.sender, key.space);
      let account_state = inputs
        .account_states
        .get(&account_key)
        .expect("account state must be provided for every pool account");

      let expected_nonce = *next_nonce
        .entry(account_key)
        .or_insert(account_state.committed_nonce);

      let balance = remaining_balance
        .entry(account_key)
        .or_insert(account_state.balance);

      let blocked = blocked_state.entry(account_key).or_insert(None);

      let state = if key.nonce < account_state.committed_nonce {
        PoolEntryState::Stale
      } else if let Some(previous_block) = *blocked {
        match previous_block {
          BlockedState::NonceGap { expected_nonce } => PoolEntryState::QueuedNonceGap {
            expected_nonce,
            actual_nonce: key.nonce,
          },
          BlockedState::InsufficientBalance {
            required,
            available,
          } => PoolEntryState::QueuedInsufficientBalance {
            required,
            available,
          },
        }
      } else if key.nonce > expected_nonce {
        let state = PoolEntryState::QueuedNonceGap {
          expected_nonce,
          actual_nonce: key.nonce,
        };
        *blocked = Some(BlockedState::NonceGap { expected_nonce });
        state
      } else {
        let cost = *inputs
          .transaction_costs
          .get(&entry.transaction.hash())
          .expect("transaction cost must be provided for every pool entry");

        if cost > *balance {
          let state = PoolEntryState::QueuedInsufficientBalance {
            required: cost,
            available: *balance,
          };
          *blocked = Some(BlockedState::InsufficientBalance {
            required: cost,
            available: *balance,
          });
          state
        } else {
          *balance -= cost;
          next_nonce.insert(
            account_key,
            expected_nonce
              .checked_add(U256::one())
              .expect("transaction nonce sequence exhausted"),
          );
          PoolEntryState::Pending
        }
      };

      states.insert(entry.transaction.hash(), state);
    }

    PoolEntryStates { states }
  }

  pub(crate) fn view(&self, view_id: H256) -> TransactionPoolView {
    let entries = self
      .by_key
      .values()
      .map(|entry| PoolViewEntry {
        transaction: Arc::clone(&entry.transaction),
        arrival_sequence: entry.arrival_sequence,
      })
      .collect();

    TransactionPoolView {
      view_id,
      pool_revision: self.revision,
      entries,
    }
  }
}
