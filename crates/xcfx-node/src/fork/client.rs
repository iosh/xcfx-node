//! Synchronous access to the fork's remote state and history.

use std::{num::NonZeroU64, sync::Arc};

use cfx_rpc_cfx_types::{
  Block, Log as CoreLog, Receipt as CoreReceipt, Transaction as CoreTransaction,
};
use cfx_rpc_eth_types::{
  Log as EthereumLog, Receipt as EthereumReceipt, Transaction as EthereumTransaction,
};
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, Space, U256};
use primitives::{StorageKeyWithSpace, block::BlockHeight};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
  execution::ExecutionCommitment,
  fee_history::{FeeHistoryRange, RewardPercentiles},
  native_state,
  rpc_client::EthereumBlock,
};

use super::{
  CoreStateQuery, EspaceStateQuery, ForkBase, ForkEpochReceipts, ForkReadError, HistoryBlockId,
  HistoryQuery, HistoryResult, StateValue, rpc::StateKey,
};

pub(super) type Reply<T> = oneshot::Sender<Result<T, ForkReadError>>;

/// Reads from one fixed remote base through a shared service.
///
/// Calls block the current thread. Use a blocking execution thread, such as
/// `spawn_blocking`, while the Tokio runtime serving requests remains active.
#[derive(Clone)]
pub(crate) struct ForkClient {
  base: Arc<ForkBase>,
  requests: mpsc::Sender<ReadRequest>,
  stopping: watch::Receiver<bool>,
}

impl ForkClient {
  pub(crate) fn core_admin_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Option<Address>, ForkReadError> {
    let HistoryResult::CoreAdmin(admin) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Admin(address),
    })?
    else {
      unreachable!("an admin query returns its declared result type");
    };

    Ok(admin)
  }

  pub(crate) fn core_sponsor_info_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<native_state::SponsorInfo, ForkReadError> {
    let HistoryResult::CoreSponsorInfo(sponsor) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::SponsorInfo(address),
    })?
    else {
      unreachable!("a sponsor-info query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(sponsor))
  }

  pub(crate) fn core_staking_balance_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(balance) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::StakingBalance(address),
    })?
    else {
      unreachable!("a staking-balance query returns its declared result type");
    };

    Ok(balance)
  }

  pub(crate) fn core_deposit_list_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Vec<primitives::DepositInfo>, ForkReadError> {
    let HistoryResult::CoreDepositList(deposits) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::DepositList(address),
    })?
    else {
      unreachable!("a deposit-list query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(deposits))
  }

  pub(crate) fn core_vote_list_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Vec<primitives::VoteStakeInfo>, ForkReadError> {
    let HistoryResult::CoreVoteList(votes) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::VoteList(address),
    })?
    else {
      unreachable!("a vote-list query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(votes))
  }

  pub(crate) fn core_collateral_for_storage_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(collateral) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::StorageCollateral(address),
    })?
    else {
      unreachable!("a collateral-for-storage query returns its declared result type");
    };

    Ok(collateral)
  }

  pub(crate) fn core_account_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<native_state::AccountInfo, ForkReadError> {
    let HistoryResult::CoreAccount(account) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Account(address),
    })?
    else {
      unreachable!("an account query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(account))
  }

  pub(crate) fn core_interest_rate_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(rate) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::InterestRate,
    })?
    else {
      unreachable!("an interest-rate query returns its declared result type");
    };

    Ok(rate)
  }

  pub(crate) fn core_accumulated_interest_rate_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(rate) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::AccumulateInterestRate,
    })?
    else {
      unreachable!("an accumulated-interest-rate query returns its declared result type");
    };

    Ok(rate)
  }

  pub(crate) fn core_pos_economics_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<native_state::PosEconomics, ForkReadError> {
    let HistoryResult::CorePosEconomics(economics) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::PosEconomics,
    })?
    else {
      unreachable!("a pos-economics query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(economics))
  }

  pub(crate) fn core_supply_info_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<native_state::TokenSupplyInfo, ForkReadError> {
    let HistoryResult::CoreSupplyInfo(supply) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::SupplyInfo,
    })?
    else {
      unreachable!("a supply-info query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(supply))
  }

  pub(crate) fn core_collateral_info_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<native_state::StorageCollateralInfo, ForkReadError> {
    let HistoryResult::CoreCollateralInfo(collateral) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::CollateralInfo,
    })?
    else {
      unreachable!("a collateral-info query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(collateral))
  }

  pub(crate) fn core_fee_burnt_at(&self, epoch_height: BlockHeight) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(fees) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::FeeBurnt,
    })?
    else {
      unreachable!("a fee-burnt query returns its declared result type");
    };

    Ok(fees)
  }

  pub(crate) fn core_vote_params_at(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<native_state::VoteParams, ForkReadError> {
    let HistoryResult::CoreVoteParams(params) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::VoteParams,
    })?
    else {
      unreachable!("a vote-params query returns its declared result type");
    };

    Ok(Arc::unwrap_or_clone(params))
  }

  pub(crate) fn core_check_balance_at(
    &self,
    request: native_state::BalanceCheckRequest,
    epoch_height: BlockHeight,
  ) -> Result<native_state::BalanceCheckResult, ForkReadError> {
    let HistoryResult::CoreBalanceCheck(check) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::CheckBalance(request),
    })?
    else {
      unreachable!("a check-balance query returns its declared result type");
    };

    Ok(check)
  }

  pub(crate) fn core_debug_epoch(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Arc<[cfx_rpc_eth_types::WrapTransaction]>, ForkReadError> {
    let HistoryResult::CoreDebugTransactions(transactions) =
      self.history(HistoryQuery::CoreDebugEpoch { epoch_height })?
    else {
      unreachable!("a Core debug request returns tagged transactions");
    };

    Ok(transactions.expect("a successful debug epoch read returns its transaction list"))
  }

  pub(crate) fn core_debug_block(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<[cfx_rpc_eth_types::WrapTransaction]>>, ForkReadError> {
    let HistoryResult::CoreDebugTransactions(transactions) =
      self.history(HistoryQuery::CoreDebugBlock { hash })?
    else {
      unreachable!("a Core debug request returns tagged transactions");
    };

    Ok(transactions)
  }

  pub(crate) fn core_fee_history(
    &self,
    block_count: NonZeroU64,
    newest_height: BlockHeight,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<Arc<FeeHistoryRange>, ForkReadError> {
    let HistoryResult::CoreFeeHistory(history) = self.history(HistoryQuery::CoreFeeHistory {
      block_count,
      newest_height,
      reward_percentiles: reward_percentiles.clone(),
    })?
    else {
      unreachable!("a Core fee history request returns a Core fee history range");
    };

    Ok(history)
  }

  pub(super) fn new(
    base: Arc<ForkBase>,
    requests: mpsc::Sender<ReadRequest>,
    stopping: watch::Receiver<bool>,
  ) -> Self {
    Self {
      base,
      requests,
      stopping,
    }
  }

  pub(crate) fn base(&self) -> &ForkBase {
    &self.base
  }

  pub(crate) fn balance(&self, address: &AddressWithSpace) -> Result<U256, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Balance {
      address: *address,
      reply,
    })?;
    self.receive(response)
  }

  pub(crate) fn state_value(
    &self,
    key: StorageKeyWithSpace<'_>,
  ) -> Result<Option<StateValue>, ForkReadError> {
    let key = StateKey::try_from(key)?;
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::StateValue { key, reply })?;
    self.receive(response)
  }

  pub(crate) fn storage_points_initialized(
    &self,
    address: &Address,
  ) -> Result<bool, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::StoragePointsInitialization {
      address: *address,
      reply,
    })?;
    self.receive(response)
  }

  pub(crate) fn pivot(&self, epoch: BlockHeight) -> Result<Arc<Block>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Pivot { epoch, reply })?;
    self.receive(response)
  }

  pub(crate) fn block(&self, epoch: BlockHeight, hash: H256) -> Result<Arc<Block>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Block { epoch, hash, reply })?;
    self.receive(response)
  }

  pub(crate) fn block_hashes(&self, epoch: BlockHeight) -> Result<Arc<[H256]>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::BlockHashes { epoch, reply })?;
    self.receive(response)
  }

  pub(crate) fn skipped_block_hashes(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Arc<[H256]>, ForkReadError> {
    let HistoryResult::CoreSkippedBlockHashes(hashes) =
      self.history(HistoryQuery::CoreSkippedBlockHashes { epoch_height })?
    else {
      unreachable!("a skipped-block request must return skipped block hashes");
    };
    Ok(hashes)
  }

  pub(crate) fn receipts(
    &self,
    epoch: BlockHeight,
    include_espace: bool,
  ) -> Result<Arc<ForkEpochReceipts>, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Receipts {
      epoch,
      include_espace,
      reply,
    })?;
    self.receive(response)
  }

  pub(crate) fn commitment(
    &self,
    epoch: BlockHeight,
  ) -> Result<ExecutionCommitment, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::Commitment { epoch, reply })?;
    self.receive(response)
  }

  /// Reads history only from the fixed remote prefix.
  ///
  /// # Errors
  ///
  /// Returns an error if a requested pivot assumption fails, required remote data
  /// is unavailable or inconsistent, or the read service cannot complete the request.
  fn history(&self, query: HistoryQuery) -> Result<HistoryResult, ForkReadError> {
    let (reply, response) = oneshot::channel();
    self.send(ReadRequest::History { query, reply })?;
    self.receive(response)
  }

  pub(crate) fn core_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<Arc<Block>>, ForkReadError> {
    let HistoryResult::CoreBlock(block) = self.history(HistoryQuery::Block {
      space: Space::Native,
      selector,
      full_transactions,
    })?
    else {
      unreachable!("a Core block request must return a Core block");
    };
    Ok(block)
  }

  pub(crate) fn core_block_with_pivot(
    &self,
    hash: H256,
    pivot: H256,
    epoch_height: BlockHeight,
  ) -> Result<Arc<Block>, ForkReadError> {
    let HistoryResult::CoreBlockWithPivot(block) =
      self.history(HistoryQuery::CoreBlockWithPivot {
        hash,
        pivot,
        epoch_height,
      })?
    else {
      unreachable!("a pivot-assumption query returns a complete Core block");
    };
    Ok(block)
  }

  pub(crate) fn core_transaction_by_hash(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<CoreTransaction>>, ForkReadError> {
    let HistoryResult::CoreTransaction(transaction) = self.history(HistoryQuery::Transaction {
      space: Space::Native,
      hash,
    })?
    else {
      unreachable!("a Core transaction request returns a Core transaction")
    };
    Ok(transaction)
  }

  pub(crate) fn core_receipt_by_hash(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<CoreReceipt>>, ForkReadError> {
    let HistoryResult::CoreReceipt(receipt) = self.history(HistoryQuery::Receipt {
      space: Space::Native,
      hash,
    })?
    else {
      unreachable!("a Core receipt request returns a Core receipt")
    };
    Ok(receipt)
  }

  pub(crate) fn core_logs(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Arc<[CoreLog]>, ForkReadError> {
    let HistoryResult::CoreLogs(logs) = self.history(HistoryQuery::Logs {
      space: Space::Native,
      epoch_height,
    })?
    else {
      unreachable!("a Core log request must return Core logs");
    };
    Ok(logs)
  }

  pub(crate) fn espace_logs(
    &self,
    epoch_height: BlockHeight,
  ) -> Result<Arc<[EthereumLog]>, ForkReadError> {
    let HistoryResult::EthereumLogs(logs) = self.history(HistoryQuery::Logs {
      space: Space::Ethereum,
      epoch_height,
    })?
    else {
      unreachable!("an eSpace log request must return eSpace logs");
    };
    Ok(logs)
  }

  /// Reads a historical eSpace block checked against its Core pivot.
  pub(crate) fn espace_block(
    &self,
    selector: HistoryBlockId,
    full_transactions: bool,
  ) -> Result<Option<Arc<EthereumBlock>>, ForkReadError> {
    let HistoryResult::EthereumBlock(block) = self.history(HistoryQuery::Block {
      space: Space::Ethereum,
      selector,
      full_transactions,
    })?
    else {
      unreachable!("an eSpace block request must return an eSpace block");
    };
    Ok(block)
  }

  /// Reads one contiguous remote fee-history segment without its next-block price.
  pub(crate) fn espace_fee_history(
    &self,
    block_count: NonZeroU64,
    newest_height: BlockHeight,
    reward_percentiles: &RewardPercentiles,
  ) -> Result<Arc<FeeHistoryRange>, ForkReadError> {
    let HistoryResult::EthereumFeeHistory(history) =
      self.history(HistoryQuery::EspaceFeeHistory {
        block_count,
        newest_height,
        reward_percentiles: reward_percentiles.clone(),
      })?
    else {
      unreachable!("an eSpace fee history request must return eSpace fee history");
    };
    Ok(history)
  }

  /// Reads a mined transaction from the fixed prefix, excluding remote pending entries.
  pub(crate) fn espace_transaction_by_hash(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<EthereumTransaction>>, ForkReadError> {
    let HistoryResult::EthereumTransaction(transaction) =
      self.history(HistoryQuery::Transaction {
        space: Space::Ethereum,
        hash,
      })?
    else {
      unreachable!("an eSpace transaction request must return an eSpace transaction");
    };
    Ok(transaction)
  }

  /// Resolves the block's hash list before reading and checking one transaction.
  pub(crate) fn espace_transaction_by_block_and_index(
    &self,
    selector: HistoryBlockId,
    index: usize,
  ) -> Result<Option<Arc<EthereumTransaction>>, ForkReadError> {
    let HistoryResult::EthereumTransaction(transaction) =
      self.history(HistoryQuery::EspaceTransactionByBlockAndIndex { selector, index })?
    else {
      unreachable!("an eSpace block transaction request must return an eSpace transaction");
    };
    Ok(transaction)
  }

  /// Reads a historical receipt with its block membership checked.
  pub(crate) fn espace_receipt(
    &self,
    hash: H256,
  ) -> Result<Option<Arc<EthereumReceipt>>, ForkReadError> {
    let HistoryResult::EthereumReceipt(receipt) = self.history(HistoryQuery::Receipt {
      space: Space::Ethereum,
      hash,
    })?
    else {
      unreachable!("an eSpace receipt request must return an eSpace receipt");
    };
    Ok(receipt)
  }

  /// Reads all receipts checked against the selected block's transaction sequence.
  pub(crate) fn espace_block_receipts(
    &self,
    selector: HistoryBlockId,
  ) -> Result<Option<Arc<[EthereumReceipt]>>, ForkReadError> {
    let HistoryResult::EthereumBlockReceipts(receipts) =
      self.history(HistoryQuery::EspaceBlockReceipts { selector })?
    else {
      unreachable!("an eSpace block receipts request must return eSpace block receipts");
    };
    Ok(receipts)
  }

  pub(crate) fn history_epoch_by_hash(
    &self,
    hash: H256,
    require_pivot: bool,
  ) -> Result<Option<BlockHeight>, ForkReadError> {
    let HistoryResult::EpochHeight(height) = self.history(HistoryQuery::EpochByHash {
      hash,
      require_pivot,
    })?
    else {
      unreachable!("a historical epoch lookup must return an epoch height");
    };
    Ok(height)
  }

  /// Reads a Core balance without reconstructing unrelated account metadata.
  pub(crate) fn core_balance_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    if epoch_height == self.base.epoch_height {
      return self.balance(&address.with_native_space());
    }
    let HistoryResult::CoreQuantity(value) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Balance(address),
    })?
    else {
      unreachable!("a Core balance query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn core_nonce_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::CoreQuantity(value) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Nonce(address),
    })?
    else {
      unreachable!("a Core nonce query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn core_code_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Arc<Vec<u8>>, ForkReadError> {
    let HistoryResult::CoreCode(value) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Code(address),
    })?
    else {
      unreachable!("a Core code query must return bytes");
    };
    Ok(value)
  }

  /// Preserves the Core RPC distinction between an absent slot and a stored word.
  pub(crate) fn core_storage_at(
    &self,
    address: Address,
    position: H256,
    epoch_height: BlockHeight,
  ) -> Result<Option<H256>, ForkReadError> {
    let HistoryResult::CoreStorage(value) = self.history(HistoryQuery::CoreState {
      epoch_height,
      query: CoreStateQuery::Storage { address, position },
    })?
    else {
      unreachable!("a Core storage query must return an optional word");
    };
    Ok(value)
  }

  pub(crate) fn espace_balance_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::EspaceQuantity(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Balance(address),
    })?
    else {
      unreachable!("a balance query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn espace_nonce_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<U256, ForkReadError> {
    let HistoryResult::EspaceQuantity(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Nonce(address),
    })?
    else {
      unreachable!("a nonce query must return a quantity");
    };
    Ok(value)
  }

  pub(crate) fn espace_code_at(
    &self,
    address: Address,
    epoch_height: BlockHeight,
  ) -> Result<Arc<Vec<u8>>, ForkReadError> {
    let HistoryResult::EspaceCode(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Code(address),
    })?
    else {
      unreachable!("a code query must return bytes");
    };
    Ok(value)
  }

  pub(crate) fn espace_storage_at(
    &self,
    address: Address,
    position: H256,
    epoch_height: BlockHeight,
  ) -> Result<H256, ForkReadError> {
    let HistoryResult::EspaceStorage(value) = self.history(HistoryQuery::EspaceState {
      epoch_height,
      query: EspaceStateQuery::Storage { address, position },
    })?
    else {
      unreachable!("a storage query must return a storage word");
    };
    Ok(value)
  }

  fn send(&self, request: ReadRequest) -> Result<(), ForkReadError> {
    if *self.stopping.borrow() {
      return Err(ForkReadError::Closed);
    }
    self
      .requests
      .blocking_send(request)
      .map_err(|_| self.disconnected_error())
  }

  fn receive<T>(
    &self,
    response: oneshot::Receiver<Result<T, ForkReadError>>,
  ) -> Result<T, ForkReadError> {
    response
      .blocking_recv()
      .map_err(|_| self.disconnected_error())?
  }

  fn disconnected_error(&self) -> ForkReadError {
    if *self.stopping.borrow() {
      ForkReadError::Closed
    } else {
      ForkReadError::ServiceStopped
    }
  }
}

pub(super) enum ReadRequest {
  History {
    query: HistoryQuery,
    reply: Reply<HistoryResult>,
  },
  Balance {
    address: AddressWithSpace,
    reply: Reply<U256>,
  },
  StateValue {
    key: StateKey,
    reply: Reply<Option<StateValue>>,
  },
  StoragePointsInitialization {
    address: Address,
    reply: Reply<bool>,
  },
  Pivot {
    epoch: BlockHeight,
    reply: Reply<Arc<Block>>,
  },
  Block {
    epoch: BlockHeight,
    hash: H256,
    reply: Reply<Arc<Block>>,
  },
  BlockHashes {
    epoch: BlockHeight,
    reply: Reply<Arc<[H256]>>,
  },
  Receipts {
    epoch: BlockHeight,
    include_espace: bool,
    reply: Reply<Arc<ForkEpochReceipts>>,
  },
  Commitment {
    epoch: BlockHeight,
    reply: Reply<ExecutionCommitment>,
  },
}
