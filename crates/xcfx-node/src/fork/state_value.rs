//! Complete state values shared by the remote cache and local fork versions.

use xcfx_state::account::Account;

use std::{mem::size_of, sync::Arc};

use primitives::{DepositInfo, DepositList, VoteStakeInfo, VoteStakeList};
use xcfx_state::database::{CodeEntry, StorageEntry};

#[derive(Clone)]
pub(crate) enum StateValue {
  Account(Arc<Account>),
  Code(CodeEntry),
  Storage(StorageEntry),
  /// The base's all-address setting hides this individual whitelist entry.
  HiddenWhitelistEntry,
  DepositList(Arc<DepositList>),
  VoteList(Arc<VoteStakeList>),
}

impl StateValue {
  /// Payload size for cache retention, excluding keys and collection overhead.
  pub(super) fn data_bytes(&self) -> usize {
    match self {
      Self::Account(_) => size_of::<Account>(),
      Self::Code(code) => size_of::<CodeEntry>() + code.code().len(),
      Self::Storage(_) => size_of::<StorageEntry>(),
      Self::HiddenWhitelistEntry => 0,
      Self::DepositList(list) => list.0.len() * size_of::<DepositInfo>(),
      Self::VoteList(list) => list.0.len() * size_of::<VoteStakeInfo>(),
    }
  }

  pub(crate) fn account(&self) -> &Account {
    let Self::Account(account) = self else {
      unreachable!("an account key must contain account data");
    };
    account
  }

  pub(crate) fn code(&self) -> &CodeEntry {
    let Self::Code(code) = self else {
      unreachable!("a code key must contain code data");
    };
    code
  }

  pub(crate) fn storage(&self) -> StorageEntry {
    let Self::Storage(entry) = self else {
      unreachable!("a storage key must contain a storage record");
    };
    *entry
  }

  pub(crate) fn deposit_list(&self) -> &DepositList {
    let Self::DepositList(list) = self else {
      unreachable!("a deposit-list key must contain a deposit list");
    };
    list
  }

  pub(crate) fn vote_list(&self) -> &VoteStakeList {
    let Self::VoteList(list) = self else {
      unreachable!("a vote-list key must contain a vote list");
    };
    list
  }
}
