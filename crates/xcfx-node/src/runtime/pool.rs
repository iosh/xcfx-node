//! Pool queries and block candidates evaluated under one Runtime state version.

mod core;

pub(crate) use core::{CorePoolStatus, PoolAccountInfo, PoolTransactionInfo};

use cfx_executor::transaction_validation::PackingCheckResult;
use xcfx_state::State;

use cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT;
use cfx_statedb::Result as StateResult;
use cfx_types::{AddressSpaceUtil, AddressWithSpace, H256, Space, U256, U512};
use primitives::{Action, Transaction};

use crate::{
  runtime_transaction::RuntimeTransaction,
  transaction_ingress::TransactionValidationContext,
  transaction_pool::{PoolTransaction, TransactionPool},
};

use super::NodeRuntime;

/// Nonce and cumulative sender-cost readiness under one Runtime state version.
/// Pending transactions still require protocol, packing-height, and budget checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PoolTransactionStatus {
  Pending,
  Queued(QueuedReason),
}

/// The first condition preventing an account's nonce sequence from advancing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QueuedReason {
  NonceGap {
    expected_nonce: U256,
  },
  /// These amounts describe `blocking_nonce`, which may precede the queried transaction.
  InsufficientBalance {
    blocking_nonce: U256,
    required: U512,
    available: U256,
  },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum PoolQueryScope {
  All,
  Space(Space),
  Account(AddressWithSpace),
}

/// Counts derived from one state version and the retained pool.
#[derive(Default)]
pub(crate) struct PoolTransactionCounts {
  pending: usize,
  queued: usize,
}

impl PoolTransactionCounts {
  pub(crate) fn pending(&self) -> usize {
    self.pending
  }

  pub(crate) fn queued(&self) -> usize {
    self.queued
  }
}

/// Owned transaction handles classified before the Runtime can change.
#[derive(Default)]
pub(crate) struct PoolContent {
  pending: Vec<RuntimeTransaction>,
  queued: Vec<RuntimeTransaction>,
}

impl PoolContent {
  pub(crate) fn into_parts(self) -> (Vec<RuntimeTransaction>, Vec<RuntimeTransaction>) {
    (self.pending, self.queued)
  }
}

/// Account transactions captured under one Runtime state and pool view.
/// The first status and total count describe candidates before the limit is applied.
#[derive(Default)]
pub(crate) struct AccountPoolTransactions {
  transactions: Vec<RuntimeTransaction>,
  first_status: Option<PoolTransactionStatus>,
  total_count: usize,
}

impl AccountPoolTransactions {
  pub(crate) fn transactions(&self) -> &[RuntimeTransaction] {
    &self.transactions
  }

  pub(crate) fn first_status(&self) -> Option<PoolTransactionStatus> {
    self.first_status
  }

  pub(crate) fn total_count(&self) -> usize {
    self.total_count
  }
}

/// Advances only within one account's strictly increasing pool nonce order.
/// The complete operations below own iteration and never expose this scan state.
struct AccountTransactionReadiness<'state, 'db> {
  state: &'state State<'db>,
  address: AddressWithSpace,
  state_nonce: U256,
  next_nonce: U256,
  remaining_balance: Option<U256>,
  queued_reason: Option<QueuedReason>,
}

impl<'state, 'db> AccountTransactionReadiness<'state, 'db> {
  fn new(state: &'state State<'db>, address: AddressWithSpace) -> StateResult<Self> {
    let state_nonce = state.nonce(&address)?;
    Ok(Self {
      state,
      address,
      state_nonce,
      next_nonce: state_nonce,
      remaining_balance: None,
      queued_reason: None,
    })
  }

  fn check_transaction(
    &mut self,
    transaction: &RuntimeTransaction,
  ) -> StateResult<PoolTransactionStatus> {
    let nonce = *transaction.nonce();
    // Admission and joint pool reconciliation exclude stale retained entries;
    // block preparation removes stale candidates before reaching this scan.
    assert!(
      nonce >= self.state_nonce,
      "classified transaction {:?} has nonce {nonce} below state nonce {} for {:?}",
      transaction.hash(),
      self.state_nonce,
      self.address,
    );
    if let Some(reason) = self.queued_reason {
      return Ok(PoolTransactionStatus::Queued(reason));
    }
    if nonce > self.next_nonce {
      let reason = QueuedReason::NonceGap {
        expected_nonce: self.next_nonce,
      };
      self.queued_reason = Some(reason);
      return Ok(PoolTransactionStatus::Queued(reason));
    }

    let available = match self.remaining_balance {
      Some(balance) => balance,
      None => self.state.balance(&self.address)?,
    };
    let cost = sender_transaction_cost(self.state, transaction)?;
    if cost > U512::from(available) {
      let reason = QueuedReason::InsufficientBalance {
        blocking_nonce: nonce,
        required: cost,
        available,
      };
      self.queued_reason = Some(reason);
      return Ok(PoolTransactionStatus::Queued(reason));
    }

    let affordable_cost =
      U256::try_from(cost).expect("a cost covered by a U256 balance must fit in U256");
    self.remaining_balance = Some(available - affordable_cost);
    self.next_nonce = nonce + U256::one();
    Ok(PoolTransactionStatus::Pending)
  }
}

/// Declared sender cost after sponsorship, preserving gas-price multiplication in U512.
fn sender_transaction_cost(
  state: &State<'_>,
  transaction: &RuntimeTransaction,
) -> StateResult<U512> {
  let mut gas_cost = transaction.gas().full_mul(*transaction.gas_price());
  let mut storage_cost = U512::from(transaction.storage_limit().unwrap_or_default())
    * U512::from(*DRIPS_PER_STORAGE_COLLATERAL_UNIT);
  if let Transaction::Native(native_transaction) = transaction.transaction()
    && let Action::Call(contract_address) = native_transaction.action()
    && state.is_contract_with_code(&contract_address.with_native_space())?
    && state.check_contract_whitelist(&contract_address, &transaction.sender())?
  {
    if gas_cost <= U512::from(state.sponsor_gas_bound(&contract_address)?)
      && gas_cost <= U512::from(state.sponsor_balance_for_gas(&contract_address)?)
    {
      gas_cost = U512::zero();
    }
    if state.sponsor_for_collateral(&contract_address)?.is_some()
      && storage_cost
        <= U512::from(state.sponsor_balance_for_collateral(&contract_address)?)
          + U512::from(state.available_storage_points_for_collateral(&contract_address)?)
    {
      storage_cost = U512::zero();
    }
  }
  Ok(U512::from(transaction.value()) + gas_cost + storage_cost)
}

/// Prepares selectable account prefixes and a deletion plan without mutating the pool.
/// Protocol checks continue after an account is blocked so invalid successors enter
/// the plan. The caller applies deletions only after a successful block commit.
pub(super) fn prepare_pool_transactions<'pool>(
  state: &State<'_>,
  pool: &'pool TransactionPool,
  validation: &TransactionValidationContext<'_>,
) -> StateResult<(Vec<&'pool PoolTransaction>, Vec<RuntimeTransaction>)> {
  let mut candidates = Vec::new();
  let mut transactions_to_drop = Vec::new();
  let mut transactions = pool.transactions().peekable();
  while let Some(first) = transactions.next() {
    let address = first.transaction().sender_with_space();
    let readiness = AccountTransactionReadiness::new(state, address)?;
    let state_nonce = readiness.state_nonce;
    let mut account_candidates = Some(readiness);
    let mut current = Some(first);
    while let Some(pooled) = current {
      let transaction = pooled.transaction();
      if *transaction.nonce() < state_nonce {
        transactions_to_drop.push(transaction.clone());
      } else {
        match validation.check_runtime_for_packing(transaction) {
          PackingCheckResult::Drop => {
            transactions_to_drop.push(transaction.clone());
            account_candidates = None;
          }
          PackingCheckResult::Pending => account_candidates = None,
          PackingCheckResult::Pack => {
            if let Some(readiness) = &mut account_candidates {
              if readiness.check_transaction(transaction)? == PoolTransactionStatus::Pending {
                candidates.push(pooled);
              } else {
                account_candidates = None;
              }
            }
          }
        }
      }
      current = transactions.next_if(|pooled| pooled.transaction().sender_with_space() == address);
    }
  }
  Ok((candidates, transactions_to_drop))
}

impl NodeRuntime {
  /// Classifies retained transactions under the effective state without copying them.
  /// A blocked account still contributes every remaining transaction to the queued count.
  ///
  /// # Errors
  /// Propagates the state reads required for nonce and cumulative sender-cost readiness.
  pub(crate) fn pool_transaction_counts(&self, space: Space) -> StateResult<PoolTransactionCounts> {
    let mut counts = PoolTransactionCounts::default();
    self.visit_pool_transactions(PoolQueryScope::Space(space), |_, status| match status {
      PoolTransactionStatus::Pending => counts.pending += 1,
      PoolTransactionStatus::Queued(_) => counts.queued += 1,
    })?;
    Ok(counts)
  }

  /// Captures complete pending and queued sets using the same classification as counts.
  /// Account scope reads only that account and Space. The pool is never pruned by a query.
  ///
  /// # Errors
  /// Propagates the state reads required for nonce and cumulative sender-cost readiness.
  pub(crate) fn pool_content(&self, scope: PoolQueryScope) -> StateResult<PoolContent> {
    let mut content = PoolContent::default();
    self.visit_pool_transactions(scope, |transaction, status| match status {
      PoolTransactionStatus::Pending => content.pending.push(transaction.clone()),
      PoolTransactionStatus::Queued(_) => content.queued.push(transaction.clone()),
    })?;
    Ok(content)
  }

  fn visit_pool_transactions(
    &self,
    scope: PoolQueryScope,
    mut visit: impl FnMut(&RuntimeTransaction, PoolTransactionStatus),
  ) -> StateResult<()> {
    let pool = &self.runtime_state.transaction_pool;
    let mut transactions: Box<dyn Iterator<Item = &PoolTransaction> + '_> = match scope {
      PoolQueryScope::All => Box::new(pool.transactions()),
      PoolQueryScope::Space(space) => Box::new(
        pool
          .transactions()
          .filter(move |pooled| pooled.transaction().space() == space),
      ),
      PoolQueryScope::Account(address) => Box::new(pool.transactions_by_account(address)),
    };
    let Some(first) = transactions.next() else {
      return Ok(());
    };

    let state = self.open_effective_state_for_reading()?;
    let mut readiness =
      AccountTransactionReadiness::new(&state, first.transaction().sender_with_space())?;
    for pooled in std::iter::once(first).chain(transactions) {
      let transaction = pooled.transaction();
      let address = transaction.sender_with_space();
      if address != readiness.address {
        readiness = AccountTransactionReadiness::new(&state, address)?;
      }
      visit(transaction, readiness.check_transaction(transaction)?);
    }
    Ok(())
  }

  pub(crate) fn pool_transactions(&self) -> impl Iterator<Item = &RuntimeTransaction> {
    self
      .runtime_state
      .transaction_pool
      .transactions()
      .map(|pooled| pooled.transaction())
  }

  /// Finds the next pool nonce from the effective state.
  ///
  /// Pool presence does not imply balance or sponsorship readiness.
  /// The returned value need not satisfy the transaction admission nonce limit.
  ///
  /// # Errors
  /// Propagates errors from reading the effective state.
  pub(crate) fn pending_nonce(&self, address: AddressWithSpace) -> StateResult<U256> {
    let state_nonce = self.open_effective_state_for_reading()?.nonce(&address)?;
    Ok(
      self
        .runtime_state
        .transaction_pool
        .next_nonce(address, state_nonce),
    )
  }

  /// Borrows a pool transaction only for the duration of this Runtime access.
  pub(crate) fn pool_transaction_by_hash(&self, hash: H256) -> Option<&RuntimeTransaction> {
    self
      .runtime_state
      .transaction_pool
      .get_by_hash(hash)
      .map(PoolTransaction::transaction)
  }

  /// Captures account transactions from `start_nonce`, defaulting to the effective state nonce.
  /// Queued transactions count toward the total before truncation. The first status includes
  /// preceding nonce and sender-cost dependencies and is retained when `limit` is zero.
  /// Later candidates are counted and captured without reading their costs.
  ///
  /// # Errors
  /// Propagates effective state and the nonce, balance, and sponsorship reads needed
  /// to determine the first status. No fee data is read beyond an account blockage.
  pub(crate) fn pool_account_transactions(
    &self,
    address: AddressWithSpace,
    start_nonce: Option<U256>,
    limit: usize,
  ) -> StateResult<AccountPoolTransactions> {
    let pool = &self.runtime_state.transaction_pool;
    // Admission and reconciliation keep retained nonces at or above the effective state nonce.
    let mut candidates = pool
      .transactions_in_nonce_range(address, start_nonce.unwrap_or_default()..=U256::max_value());
    let Some(first) = candidates.next() else {
      return Ok(AccountPoolTransactions::default());
    };

    let state = self.open_effective_state_for_reading()?;
    let mut readiness = AccountTransactionReadiness::new(&state, address)?;
    let first_nonce = *first.transaction().nonce();
    for pooled in pool
      .transactions_in_nonce_range(address, readiness.state_nonce..=first_nonce)
      .take_while(|pooled| *pooled.transaction().nonce() < first_nonce)
    {
      if matches!(
        readiness.check_transaction(pooled.transaction())?,
        PoolTransactionStatus::Queued(_)
      ) {
        break;
      }
    }
    let first_status = readiness.check_transaction(first.transaction())?;

    let mut transactions = Vec::new();
    let mut total_count = 0;
    for pooled in std::iter::once(first).chain(candidates) {
      total_count += 1;
      if transactions.len() < limit {
        transactions.push(pooled.transaction().clone());
      }
    }
    Ok(AccountPoolTransactions {
      transactions,
      first_status: Some(first_status),
      total_count,
    })
  }
}
