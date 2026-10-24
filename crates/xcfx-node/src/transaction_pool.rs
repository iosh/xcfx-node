use std::{
  collections::{BTreeMap, BTreeSet, HashMap, btree_map::Entry},
  ops::RangeInclusive,
};

use crate::runtime_transaction::RuntimeTransaction;
use cfx_types::{Address, AddressWithSpace, H256, Space, U256};
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

/// An admitted transaction and its deterministic arrival order in the pool.
/// Its status is derived from Runtime state, rather than stored in the pool.
#[derive(Clone)]
pub(crate) struct PoolTransaction {
  transaction: RuntimeTransaction,
  arrival_sequence: u64,
}

impl PoolTransaction {
  pub(crate) fn transaction(&self) -> &RuntimeTransaction {
    &self.transaction
  }

  pub(crate) fn arrival_sequence(&self) -> u64 {
    self.arrival_sequence
  }
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

pub(crate) struct TransactionPoolCheckpoint {
  transactions: Vec<PoolTransaction>,
  next_arrival_sequence: u64,
}

pub(crate) struct TransactionPoolReconciliation {
  transaction_hashes_to_remove: Vec<H256>,
}

#[derive(Clone)]
pub(crate) struct TransactionPool {
  policy: TransactionPoolPolicy,
  by_key: BTreeMap<TransactionKey, PoolTransaction>,
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
      transactions: self.by_key.values().cloned().collect(),
      next_arrival_sequence: self.next_arrival_sequence,
    }
  }

  /// Restores entries captured under the same instance policy and rebuilds the indexes.
  pub(crate) fn from_checkpoint(
    policy: TransactionPoolPolicy,
    checkpoint: &TransactionPoolCheckpoint,
  ) -> Self {
    let mut pool = Self::new(policy);

    pool.key_by_hash.reserve(checkpoint.transactions.len());
    pool.next_arrival_sequence = checkpoint.next_arrival_sequence;

    for pooled in &checkpoint.transactions {
      let key = TransactionKey::from_transaction(&pooled.transaction);
      let hash = pooled.transaction.hash();

      pool.by_key.insert(key, pooled.clone());
      pool.key_by_hash.insert(hash, key);
    }

    pool
  }

  pub(crate) fn len(&self) -> usize {
    self.by_key.len()
  }

  pub(crate) fn get_by_hash(&self, hash: H256) -> Option<&PoolTransaction> {
    self
      .key_by_hash
      .get(&hash)
      .and_then(|key| self.by_key.get(key))
  }

  pub(crate) fn get_by_key(&self, key: &TransactionKey) -> Option<&PoolTransaction> {
    self.by_key.get(key)
  }

  /// Borrows transactions in `(sender, space, nonce)` order.
  pub(crate) fn transactions(
    &self,
  ) -> impl DoubleEndedIterator<Item = &PoolTransaction> + ExactSizeIterator {
    self.by_key.values()
  }

  /// Borrows one account and Space in ascending nonce order.
  pub(crate) fn transactions_by_account(
    &self,
    address: AddressWithSpace,
  ) -> impl DoubleEndedIterator<Item = &PoolTransaction> {
    self.transactions_in_nonce_range(address, U256::zero()..=U256::max_value())
  }

  /// Borrows an inclusive nonce range. Reversed bounds yield an empty iterator.
  pub(crate) fn transactions_in_nonce_range(
    &self,
    address: AddressWithSpace,
    nonces: RangeInclusive<U256>,
  ) -> impl DoubleEndedIterator<Item = &PoolTransaction> {
    let (start, end) = nonces.into_inner();
    let first = TransactionKey {
      sender: address.address,
      space: address.space,
      nonce: start,
    };
    let last = TransactionKey {
      nonce: end,
      ..first
    };

    let transactions = if start > end {
      self.by_key.range(first..first)
    } else {
      self.by_key.range(first..=last)
    };
    transactions.map(|(_, transaction)| transaction)
  }

  /// Finds the first unoccupied nonce at or above `state_nonce`.
  /// Admitted transaction nonces are below `u64::MAX`, leaving room to advance.
  pub(crate) fn next_nonce(&self, address: AddressWithSpace, state_nonce: U256) -> U256 {
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
      nonce += U256::one();
    }

    nonce
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
    let at_capacity = self.by_key.len() >= self.policy.max_transactions;
    match self.by_key.entry(key) {
      Entry::Occupied(mut entry) => {
        if matches!(replacement_policy, ReplacementPolicy::RequirePriceBump)
          && !Self::replacement_is_sufficient(&transaction, &entry.get().transaction)
        {
          return Err(TransactionError::TooCheapToReplace);
        }

        let arrival_sequence = Self::next_arrival_sequence(&mut self.next_arrival_sequence);
        let previous = entry.insert(PoolTransaction {
          transaction,
          arrival_sequence,
        });
        self
          .key_by_hash
          .remove(&previous.transaction.hash())
          .expect("pool hash index must match primary index");
        self.key_by_hash.insert(hash, key);
        Ok(TransactionPoolInsertOutcome::Replaced {
          previous: previous.transaction,
        })
      }
      Entry::Vacant(entry) => {
        if at_capacity {
          return Err(TransactionError::LimitReached);
        }

        let arrival_sequence = Self::next_arrival_sequence(&mut self.next_arrival_sequence);
        entry.insert(PoolTransaction {
          transaction,
          arrival_sequence,
        });
        self.key_by_hash.insert(hash, key);
        Ok(TransactionPoolInsertOutcome::Inserted)
      }
    }
  }

  fn next_arrival_sequence(next: &mut u64) -> u64 {
    let sequence = *next;
    *next = sequence
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
}
