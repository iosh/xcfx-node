use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::runtime_transaction::RuntimeTransaction;
use cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT;
use cfx_types::{Address, AddressWithSpace, H256, Space, U256, U512};
use primitives::{Account, transaction::TransactionError};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TransactionKey {
  sender: Address,
  space: Space,
  nonce: U256,
}

impl TransactionKey {
  pub(crate) fn from_transaction(transaction: &RuntimeTransaction) -> Self {
    Self {
      sender: transaction.sender(),
      space: transaction.space(),
      nonce: *transaction.nonce(),
    }
  }
}

pub(crate) type AccountKey = (Address, Space);

pub(crate) struct PoolAccountState {
  pub(crate) state_nonce: U256,
  pub(crate) balance: U256,
}

impl PoolAccountState {
  pub(crate) fn from_account(account: &Account) -> Self {
    Self {
      state_nonce: account.nonce,
      balance: account.balance,
    }
  }
}

/// Maximum declared sender cost after sponsorship, without truncating user inputs.
/// U512 preserves gas-price multiplication even when no U256 balance can cover it.
pub(crate) fn pool_transaction_cost(
  transaction: &RuntimeTransaction,
  sponsored_gas: U256,
  sponsored_storage: u64,
) -> U512 {
  let gas = *transaction.gas() - sponsored_gas;
  let gas_cost = gas.full_mul(*transaction.gas_price());

  let storage_cost = transaction
    .storage_limit()
    .map_or_else(U256::zero, |limit| {
      let unsponsored_storage = limit
        .checked_sub(sponsored_storage)
        .expect("sponsored storage cannot exceed transaction storage limit");

      U256::from(unsponsored_storage) * *DRIPS_PER_STORAGE_COLLATERAL_UNIT
    });

  U512::from(transaction.value()) + gas_cost + U512::from(storage_cost)
}

pub(crate) struct PoolReadinessInputs {
  pub(crate) account_states: BTreeMap<AccountKey, PoolAccountState>,
  pub(crate) transaction_costs: BTreeMap<H256, U512>,
}

impl PoolReadinessInputs {
  pub(crate) fn new(
    account_states: BTreeMap<AccountKey, PoolAccountState>,
    transaction_costs: BTreeMap<H256, U512>,
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
    required: U512,
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
  InsufficientBalance { required: U512, available: U256 },
}

#[derive(Clone)]
pub(crate) struct PoolEntry {
  pub(crate) transaction: RuntimeTransaction,
  pub(crate) arrival_sequence: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
  Replaced { previous: RuntimeTransaction },
}

pub(crate) struct PoolViewEntry {
  pub(crate) transaction: RuntimeTransaction,
  pub(crate) arrival_sequence: u64,
}

pub(crate) struct TransactionPoolView {
  /// Entries preserve the pool's `(sender, space, nonce)` key order.
  pub(crate) entries: Vec<PoolViewEntry>,
}

pub(crate) struct TransactionPoolCheckpoint {
  entries: Vec<PoolEntry>,
  next_arrival_sequence: u64,
}

pub(crate) struct PoolSelectionInput {
  pub(crate) view: TransactionPoolView,
  pub(crate) entry_states: PoolEntryStates,
}

pub(crate) struct TransactionPoolReconciliation {
  transaction_hashes_to_remove: Vec<H256>,
}

pub(crate) struct TransactionPool {
  policy: TransactionPoolPolicy,
  by_key: BTreeMap<TransactionKey, PoolEntry>,
  key_by_hash: HashMap<H256, TransactionKey>,
  next_arrival_sequence: u64,
}

enum ReplacementPolicy {
  RequirePriceBump,
  ReplaceExisting,
}

impl TransactionPool {
  pub(crate) fn new(policy: TransactionPoolPolicy) -> Self {
    Self {
      policy,
      by_key: BTreeMap::new(),
      key_by_hash: HashMap::new(),
      next_arrival_sequence: 0,
    }
  }

  pub(crate) fn capture_checkpoint(&self) -> TransactionPoolCheckpoint {
    TransactionPoolCheckpoint {
      entries: self.by_key.values().cloned().collect(),
      next_arrival_sequence: self.next_arrival_sequence,
    }
  }

  pub(crate) fn from_checkpoint(
    policy: TransactionPoolPolicy,
    checkpoint: &TransactionPoolCheckpoint,
  ) -> Self {
    assert!(
      checkpoint.entries.len() <= policy.max_transactions,
      "a checkpoint cannot restore more transactions than the stable pool policy allows",
    );

    let mut pool = Self::new(policy);
    let mut arrival_sequences = BTreeSet::new();

    pool.key_by_hash.reserve(checkpoint.entries.len());
    pool.next_arrival_sequence = checkpoint.next_arrival_sequence;

    for entry in &checkpoint.entries {
      assert!(
        entry.arrival_sequence < checkpoint.next_arrival_sequence,
        "a checkpoint pool entry must have an allocated arrival sequence",
      );
      assert!(
        arrival_sequences.insert(entry.arrival_sequence),
        "checkpoint pool entries must have distinct arrival sequences",
      );

      let key = TransactionKey::from_transaction(&entry.transaction);
      let hash = entry.transaction.hash();

      assert!(
        pool.by_key.insert(key, entry.clone()).is_none(),
        "checkpoint pool entries must have distinct conflict keys",
      );
      assert!(
        pool.key_by_hash.insert(hash, key).is_none(),
        "checkpoint pool entries must have distinct transaction hashes",
      );
    }

    pool
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

  /// Finds the first unoccupied nonce at or above `state_nonce`.
  /// Returns `None` if the contiguous sequence exhausts the `U256` range.
  pub(crate) fn next_nonce(&self, address: AddressWithSpace, state_nonce: U256) -> Option<U256> {
    let first = TransactionKey {
      sender: address.address,
      space: address.space,
      nonce: state_nonce,
    };
    let last = TransactionKey {
      nonce: U256::max_value(),
      ..first
    };

    let mut nonce = state_nonce;
    for (key, _) in self.by_key.range(first..=last) {
      if key.nonce != nonce {
        break;
      }
      nonce = nonce.checked_add(U256::one())?;
    }

    Some(nonce)
  }

  pub(crate) fn insert(
    &mut self,
    transaction: RuntimeTransaction,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    self.insert_with_replacement_policy(transaction, ReplacementPolicy::RequirePriceBump)
  }

  /// Reinserts a transaction revalidated against the new execution view after a reorg.
  ///
  /// Same-key replacement does not require a gas-price bump. Duplicate hashes
  /// and the pool's capacity limit are still enforced.
  pub(crate) fn reinsert_after_reorg(
    &mut self,
    transaction: RuntimeTransaction,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    self.insert_with_replacement_policy(transaction, ReplacementPolicy::ReplaceExisting)
  }

  fn insert_with_replacement_policy(
    &mut self,
    transaction: RuntimeTransaction,
    replacement_policy: ReplacementPolicy,
  ) -> Result<TransactionPoolInsertOutcome, TransactionError> {
    assert!(
      !matches!(&transaction, RuntimeTransaction::System(_)),
      "system transactions must not enter the user transaction pool",
    );

    let hash = transaction.hash();

    if self.key_by_hash.contains_key(&hash) {
      return Err(TransactionError::AlreadyImported);
    }
    let key = TransactionKey::from_transaction(&transaction);
    if let Some((previous, previous_hash)) = self
      .by_key
      .get(&key)
      .map(|entry| (entry.transaction.clone(), entry.transaction.hash()))
    {
      if matches!(replacement_policy, ReplacementPolicy::RequirePriceBump)
        && !Self::replacement_is_sufficient(&transaction, &previous)
      {
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

      return Ok(TransactionPoolInsertOutcome::Replaced { previous });
    }

    if self.by_key.len() >= self.policy.max_transactions {
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

    Ok(TransactionPoolInsertOutcome::Inserted)
  }

  fn next_arrival_sequence(&mut self) -> u64 {
    let sequence = self.next_arrival_sequence;
    self.next_arrival_sequence = sequence
      .checked_add(1)
      .expect("transaction pool arrival sequence exhausted");
    sequence
  }

  fn replacement_is_sufficient(
    replacement: &RuntimeTransaction,
    previous: &RuntimeTransaction,
  ) -> bool {
    let previous_price = *previous.gas_price();
    let required_price = if previous_price < U256::from(100) {
      previous_price.checked_add(U256::one())
    } else {
      (previous_price / U256::from(100))
        .checked_mul(U256::from(2))
        .and_then(|bump| previous_price.checked_add(bump))
    };

    required_price.is_some_and(|required| *replacement.gas_price() >= required)
  }

  pub(crate) fn remove_by_hash(&mut self, hash: H256) -> Option<RuntimeTransaction> {
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

  pub(crate) fn prepare_reconciliation(
    &self,
    transactions_to_remove: impl IntoIterator<Item = H256>,
    modified_accounts: &[Account],
  ) -> TransactionPoolReconciliation {
    let account_nonces = modified_accounts.iter().map(|account| {
      let address = account.address();
      ((address.address, address.space), account.nonce)
    });

    self.prepare_nonce_reconciliation(transactions_to_remove, account_nonces)
  }

  pub(crate) fn prepare_nonce_reconciliation(
    &self,
    transactions_to_remove: impl IntoIterator<Item = H256>,
    account_nonces: impl IntoIterator<Item = (AccountKey, U256)>,
  ) -> TransactionPoolReconciliation {
    let mut transaction_hashes_to_remove =
      transactions_to_remove.into_iter().collect::<BTreeSet<_>>();

    for ((sender, space), nonce) in account_nonces {
      let first_key = TransactionKey {
        sender,
        space,
        nonce: U256::zero(),
      };
      let nonce_key = TransactionKey {
        sender,
        space,
        nonce,
      };

      for entry in self
        .by_key
        .range(first_key..nonce_key)
        .map(|(_, entry)| entry)
      {
        transaction_hashes_to_remove.insert(entry.transaction.hash());
      }
    }

    TransactionPoolReconciliation {
      transaction_hashes_to_remove: transaction_hashes_to_remove.into_iter().collect(),
    }
  }

  pub(crate) fn apply_reconciliation(&mut self, reconciliation: TransactionPoolReconciliation) {
    for hash in reconciliation.transaction_hashes_to_remove {
      let _ = self.remove_by_hash(hash);
    }
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
        .or_insert(account_state.state_nonce);

      let balance = remaining_balance
        .entry(account_key)
        .or_insert(account_state.balance);

      let blocked = blocked_state.entry(account_key).or_insert(None);

      let state = if key.nonce < account_state.state_nonce {
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

        if cost > U512::from(*balance) {
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
          let affordable_cost =
            U256::try_from(cost).expect("a cost covered by a U256 balance must fit in U256");
          *balance -= affordable_cost;
          // Unique, ordered keys make U256::MAX this account's final entry.
          if let Some(nonce) = expected_nonce.checked_add(U256::one()) {
            next_nonce.insert(account_key, nonce);
          }
          PoolEntryState::Pending
        }
      };

      states.insert(entry.transaction.hash(), state);
    }

    PoolEntryStates { states }
  }

  pub(crate) fn view(&self) -> TransactionPoolView {
    let entries = self
      .by_key
      .values()
      .map(|entry| PoolViewEntry {
        transaction: entry.transaction.clone(),
        arrival_sequence: entry.arrival_sequence,
      })
      .collect();

    TransactionPoolView { entries }
  }
}
