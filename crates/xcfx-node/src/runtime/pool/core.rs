//! Core pool diagnostics use the same effective state as transaction selection.

use cfx_statedb::Result as StateResult;
use cfx_types::{AddressWithSpace, H256, U256, U512};

use crate::runtime_transaction::RuntimeTransaction;

use super::{NodeRuntime, PoolQueryScope, PoolTransactionStatus};

pub(crate) struct CorePoolStatus {
  ready_accounts: usize,
  retained_transactions: usize,
}

impl CorePoolStatus {
  pub(crate) fn into_parts(self) -> (usize, usize) {
    (self.ready_accounts, self.retained_transactions)
  }
}

pub(crate) struct PoolAccountInfo {
  state_nonce: U256,
  pending_count: usize,
  first_pending: Option<(U256, H256)>,
}

impl PoolAccountInfo {
  pub(crate) fn into_parts(self) -> (U256, usize, Option<(U256, H256)>) {
    (self.state_nonce, self.pending_count, self.first_pending)
  }
}

/// Pool reconciliation and state publication leave no separate stale account cache.
pub(crate) struct PoolTransactionInfo {
  nonce: U256,
  balance: U256,
  balance_enough: bool,
}

impl PoolTransactionInfo {
  pub(crate) fn into_parts(self) -> (U256, U256, bool) {
    (self.nonce, self.balance, self.balance_enough)
  }
}

impl NodeRuntime {
  pub(crate) fn core_pool_status(&self) -> StateResult<CorePoolStatus> {
    let mut ready_accounts = 0;
    let mut previous_ready_account = None;

    self.visit_pool_transactions(PoolQueryScope::All, |transaction, status| {
      let account = transaction.sender_with_space();

      if status == PoolTransactionStatus::Pending && previous_ready_account != Some(account) {
        ready_accounts += 1;
        previous_ready_account = Some(account);
      }
    })?;

    Ok(CorePoolStatus {
      ready_accounts,
      retained_transactions: self.runtime_state.transaction_pool.len(),
    })
  }

  pub(crate) fn pool_transaction_at_nonce(
    &self,
    address: AddressWithSpace,
    nonce: U256,
  ) -> Option<&RuntimeTransaction> {
    self
      .runtime_state
      .transaction_pool
      .transactions_in_nonce_range(address, nonce..=nonce)
      .next()
      .map(|pooled| pooled.transaction())
  }

  pub(crate) fn pool_pending_nonce_range(&self, address: AddressWithSpace) -> Option<(U256, U256)> {
    let mut transactions = self
      .runtime_state
      .transaction_pool
      .transactions_by_account(address);

    let first = transactions.next()?.transaction();
    let last = transactions
      .next_back()
      .map_or(first, |pooled| pooled.transaction());

    Some((*first.nonce(), *last.nonce()))
  }

  pub(crate) fn pool_account_info(
    &self,
    address: AddressWithSpace,
  ) -> StateResult<PoolAccountInfo> {
    let state_nonce = self.open_effective_state_for_reading()?.nonce(&address)?;
    let mut transactions = self
      .runtime_state
      .transaction_pool
      .transactions_by_account(address);

    let first_pending = transactions.next().map(|pooled| {
      let transaction = pooled.transaction();
      (*transaction.nonce(), transaction.hash())
    });

    let pending_count = usize::from(first_pending.is_some()) + transactions.count();

    Ok(PoolAccountInfo {
      state_nonce,
      pending_count,
      first_pending,
    })
  }

  pub(crate) fn pool_transaction_info(
    &self,
    hash: H256,
  ) -> StateResult<Option<PoolTransactionInfo>> {
    let Some(transaction) = self.pool_transaction_by_hash(hash) else {
      return Ok(None);
    };

    let address = transaction.sender_with_space();
    let state = self.open_effective_state_for_reading()?;
    let nonce = state.nonce(&address)?;
    let balance = state.balance(&address)?;

    // This RPC reports the declared unsponsored cost, using a strict comparison.
    let required = U512::from(transaction.value())
      + transaction.gas().full_mul(*transaction.gas_price())
      + U512::from(transaction.storage_limit().unwrap_or_default())
        * U512::from(*cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT);

    Ok(Some(PoolTransactionInfo {
      nonce,
      balance,
      balance_enough: U512::from(balance) > required,
    }))
  }

  /// Removes both Spaces without publishing a chain change or scheduling a block.
  pub(crate) fn drop_all_transactions(&mut self) {
    self.runtime_state.transaction_pool.clear();
  }
}
