//! Complete Core operations executed and captured on the worker.

mod pool;

use std::sync::Arc;

use cfx_addr::Network;
use cfx_rpc_cfx_types::{
  Block, Receipt, RpcAddress, Transaction,
  address::{UnexpectedRpcAddressNetwork, check_rpc_address_network},
};
use cfx_types::{Address, AddressSpaceUtil, H256, Space, U256};
use thiserror::Error;

use crate::{
  block_producer::RuntimeBlock,
  chain::{MinedBlockView, MinedTransactionView, TransactionReceiptView},
  fee_history::{FeeHistory, RewardPercentiles},
  fork::{ForkEpochReceipts, ForkReadError},
  logs::network,
  native_state,
  runtime::{
    CoreBlockSelectionError, CoreBlockSelector, CoreBlockSource, CoreBlockWithPivotSource,
    CoreDebugTransactionsSource, CoreEpochQueryError, CoreEpochReceiptsSource, CoreReceiptSource,
    CoreStorageRootError, CoreTransactionSource, EpochSelector, FeeQueryError,
    RuntimeSimulationError, RuntimeTransactionError, StateQueryError, StateSelectionError,
    StateSelector,
  },
  runtime_transaction::RuntimeTransaction,
  signing::MessageSigningError,
  transaction_request::{NativeTransactionRequest, TransactionRequest},
  virtual_execution::TransactionEstimate,
};

use super::NodeCall;

pub(crate) enum CoreBlock {
  Local(MinedBlockView),
  Detached(Arc<RuntimeBlock>),
  Fork(Arc<Block>),
}

pub(crate) enum CoreTransaction {
  Pool(RuntimeTransaction),
  Local(MinedTransactionView),
  Fork(Arc<Transaction>),
}

pub(crate) enum CoreReceipt {
  Local(TransactionReceiptView),
  Fork(Arc<Receipt>),
}

pub(crate) enum CoreEpochReceipts {
  Local(Vec<MinedBlockView>),
  Fork(Arc<ForkEpochReceipts>),
}

pub(crate) enum CoreDebugTransactions {
  Local(Vec<MinedBlockView>),
  Fork(Arc<[cfx_rpc_eth_types::WrapTransaction]>),
}

#[derive(Debug, Error)]
pub(crate) enum CoreBlockQueryError {
  #[error(transparent)]
  Selection(#[from] CoreBlockSelectionError),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
}

#[derive(Debug, Error)]
pub(crate) enum CoreEpochReceiptsError {
  #[error(transparent)]
  Selection(#[from] StateSelectionError),
  #[error(transparent)]
  Fork(#[from] ForkReadError),
}

#[derive(Debug, Error)]
pub(crate) enum CoreStateQueryError<E = StateQueryError> {
  #[error("{0}")]
  Network(UnexpectedRpcAddressNetwork),
  #[error(transparent)]
  Query(#[from] E),
}

#[derive(Debug, Error)]
pub(crate) enum CoreOperationError<E> {
  #[error("{0}")]
  Network(UnexpectedRpcAddressNetwork),
  #[error("{error}")]
  Operation {
    #[source]
    error: E,
    network: Network,
  },
}

fn capture_block(
  source: Option<CoreBlockSource<'_>>,
  full: bool,
) -> Result<Option<CoreBlock>, ForkReadError> {
  Ok(match source {
    None => None,
    Some(CoreBlockSource::Local(block)) => Some(CoreBlock::Local(block)),
    Some(CoreBlockSource::Detached(block)) => Some(CoreBlock::Detached(block)),
    Some(CoreBlockSource::Fork { client, selector }) => {
      client.core_block(selector, full)?.map(CoreBlock::Fork)
    }
  })
}

fn capture_debug_transactions(
  source: CoreDebugTransactionsSource<'_>,
) -> Result<Option<CoreDebugTransactions>, ForkReadError> {
  Ok(match source {
    CoreDebugTransactionsSource::Local(blocks) => Some(CoreDebugTransactions::Local(blocks)),
    CoreDebugTransactionsSource::ForkEpoch {
      client,
      epoch_height,
    } => Some(CoreDebugTransactions::Fork(
      client.core_debug_epoch(epoch_height)?,
    )),
    CoreDebugTransactionsSource::ForkBlock { client, hash } => client
      .core_debug_block(hash)?
      .map(CoreDebugTransactions::Fork),
  })
}

impl NodeCall<'_, '_> {
  pub(crate) fn core_balance_at(
    self,
    address: RpcAddress,
    selector: StateSelector,
  ) -> Result<U256, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    Ok(
      self
        .worker
        .node
        .runtime()
        .core_balance_at(address, selector)?,
    )
  }

  pub(crate) fn core_nonce_at(
    self,
    address: RpcAddress,
    selector: StateSelector,
  ) -> Result<U256, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    Ok(
      self
        .worker
        .node
        .runtime()
        .core_nonce_at(address, selector)?,
    )
  }

  pub(crate) fn core_code_at(
    self,
    address: RpcAddress,
    selector: StateSelector,
  ) -> Result<Arc<Vec<u8>>, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    Ok(self.worker.node.runtime().core_code_at(address, selector)?)
  }

  pub(crate) fn core_storage_at(
    self,
    address: RpcAddress,
    position: H256,
    selector: StateSelector,
  ) -> Result<Option<H256>, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    Ok(
      self
        .worker
        .node
        .runtime()
        .core_storage_at(address, position, selector)?,
    )
  }

  pub(crate) fn core_admin_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<(Option<Address>, Network), CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let admin = runtime.core_admin_at(address, selector)?;

    Ok((admin, network(runtime.network_id())))
  }

  pub(crate) fn core_sponsor_info_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<(native_state::SponsorInfo, Network), CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let sponsor = runtime.core_sponsor_info_at(address, selector)?;

    Ok((sponsor, network(runtime.network_id())))
  }

  pub(crate) fn core_staking_balance_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<U256, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let balance = runtime.core_staking_balance_at(address, selector)?;

    Ok(balance)
  }

  pub(crate) fn core_deposit_list_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<Vec<primitives::DepositInfo>, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let deposits = runtime.core_deposit_list_at(address, selector)?;

    Ok(deposits)
  }

  pub(crate) fn core_vote_list_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<Vec<primitives::VoteStakeInfo>, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let votes = runtime.core_vote_list_at(address, selector)?;

    Ok(votes)
  }

  pub(crate) fn core_collateral_for_storage_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<U256, CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let collateral = runtime.core_collateral_for_storage_at(address, selector)?;

    Ok(collateral)
  }

  pub(crate) fn core_account_at(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<(native_state::AccountInfo, Network), CoreStateQueryError> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    let runtime = self.worker.node.runtime();
    let account = runtime.core_account_at(address, selector)?;

    Ok((account, network(runtime.network_id())))
  }

  pub(crate) fn core_interest_rate_at(
    self,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    self.worker.node.runtime().core_interest_rate_at(selector)
  }

  pub(crate) fn core_accumulated_interest_rate_at(
    self,
    selector: EpochSelector,
  ) -> Result<U256, StateQueryError> {
    self
      .worker
      .node
      .runtime()
      .core_accumulated_interest_rate_at(selector)
  }

  pub(crate) fn core_pos_economics_at(
    self,
    selector: EpochSelector,
  ) -> Result<native_state::PosEconomics, StateQueryError> {
    self.worker.node.runtime().core_pos_economics_at(selector)
  }

  pub(crate) fn core_supply_info_at(
    self,
    selector: EpochSelector,
  ) -> Result<native_state::TokenSupplyInfo, StateQueryError> {
    self.worker.node.runtime().core_supply_info_at(selector)
  }

  pub(crate) fn core_collateral_info_at(
    self,
    selector: EpochSelector,
  ) -> Result<native_state::StorageCollateralInfo, StateQueryError> {
    self.worker.node.runtime().core_collateral_info_at(selector)
  }

  pub(crate) fn core_fee_burnt_at(self, selector: EpochSelector) -> Result<U256, StateQueryError> {
    self.worker.node.runtime().core_fee_burnt_at(selector)
  }

  pub(crate) fn core_vote_params_at(
    self,
    selector: EpochSelector,
  ) -> Result<native_state::VoteParams, StateQueryError> {
    self.worker.node.runtime().core_vote_params_at(selector)
  }

  pub(crate) fn core_check_balance_at(
    self,
    account: RpcAddress,
    contract: RpcAddress,
    gas_limit: U256,
    gas_price: U256,
    storage_limit: u64,
    selector: EpochSelector,
  ) -> Result<native_state::BalanceCheckResult, CoreStateQueryError> {
    let account = self
      .native_address(account)
      .map_err(CoreStateQueryError::Network)?;
    let contract = self
      .native_address(contract)
      .map_err(CoreStateQueryError::Network)?;
    let request = native_state::BalanceCheckRequest {
      account,
      contract,
      gas_limit,
      gas_price,
      storage_limit,
    };

    Ok(
      self
        .worker
        .node
        .runtime()
        .core_check_balance_at(request, selector)?,
    )
  }

  pub(crate) fn core_storage_root(
    self,
    address: RpcAddress,
    selector: EpochSelector,
  ) -> Result<primitives::StorageRoot, CoreStateQueryError<CoreStorageRootError>> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;
    Ok(
      self
        .worker
        .node
        .runtime()
        .core_storage_root(address, selector)?,
    )
  }

  pub(crate) fn core_status(self) -> crate::runtime::ChainStatus {
    self.worker.node.runtime().chain_status()
  }

  pub(crate) fn core_confirmation_risk(
    self,
    hash: H256,
  ) -> Result<Option<cfx_types::U256>, ForkReadError> {
    self.worker.node.runtime().core_confirmation_risk(hash)
  }

  pub(crate) fn core_block(
    self,
    selector: CoreBlockSelector,
    full: bool,
  ) -> Result<(Option<CoreBlock>, Network), CoreEpochQueryError> {
    let runtime = self.worker.node.runtime();
    Ok((
      capture_block(runtime.core_block_source(selector)?, full)?,
      network(runtime.network_id()),
    ))
  }

  pub(crate) fn core_block_with_pivot(
    self,
    hash: H256,
    pivot: H256,
    epoch_height: u64,
  ) -> Result<(CoreBlock, Network), CoreBlockQueryError> {
    let runtime = self.worker.node.runtime();
    let block = match runtime.core_block_with_pivot_source(hash, pivot, epoch_height)? {
      CoreBlockWithPivotSource::Local(block) => CoreBlock::Local(block),
      CoreBlockWithPivotSource::Fork {
        client,
        hash,
        pivot,
        epoch_height,
      } => CoreBlock::Fork(client.core_block_with_pivot(hash, pivot, epoch_height)?),
    };
    Ok((block, network(runtime.network_id())))
  }

  pub(crate) fn core_transaction_by_hash(
    self,
    hash: H256,
  ) -> Result<(Option<CoreTransaction>, Network), ForkReadError> {
    let runtime = self.worker.node.runtime();
    let transaction = match runtime.core_transaction_source(hash) {
      None => None,
      Some(CoreTransactionSource::Pool(transaction)) => {
        Some(CoreTransaction::Pool(transaction.clone()))
      }
      Some(CoreTransactionSource::Local(transaction)) => Some(CoreTransaction::Local(transaction)),
      Some(CoreTransactionSource::Fork { client, hash }) => client
        .core_transaction_by_hash(hash)?
        .map(CoreTransaction::Fork),
    };
    Ok((transaction, network(runtime.network_id())))
  }

  pub(crate) fn core_transaction_receipt(
    self,
    hash: H256,
  ) -> Result<(Option<CoreReceipt>, Network), ForkReadError> {
    let runtime = self.worker.node.runtime();
    let receipt = match runtime.core_receipt_source(hash) {
      None => None,
      Some(CoreReceiptSource::Local(receipt)) => Some(CoreReceipt::Local(receipt)),
      Some(CoreReceiptSource::Fork { client, hash }) => {
        client.core_receipt_by_hash(hash)?.map(CoreReceipt::Fork)
      }
    };
    Ok((receipt, network(runtime.network_id())))
  }

  pub(crate) fn core_epoch_receipts(
    self,
    selector: StateSelector,
    include_espace: bool,
  ) -> Result<(Option<CoreEpochReceipts>, Network), CoreEpochReceiptsError> {
    let runtime = self.worker.node.runtime();
    let receipts = match runtime.core_epoch_receipts_source(selector)? {
      None => None,
      Some(CoreEpochReceiptsSource::Local(blocks)) => Some(CoreEpochReceipts::Local(blocks)),
      Some(CoreEpochReceiptsSource::Fork {
        client,
        epoch_height,
      }) => Some(CoreEpochReceipts::Fork(
        client.receipts(epoch_height, include_espace)?,
      )),
    };
    Ok((receipts, network(runtime.network_id())))
  }

  pub(crate) fn core_debug_epoch(
    self,
    epoch_height: u64,
  ) -> Result<(CoreDebugTransactions, Network), CoreEpochQueryError> {
    let runtime = self.worker.node.runtime();
    let source = runtime.core_debug_epoch_source(epoch_height)?;
    let transactions = capture_debug_transactions(source)?
      .expect("a selected debug epoch returns its transaction list");

    Ok((transactions, network(runtime.network_id())))
  }

  pub(crate) fn core_debug_block(
    self,
    hash: H256,
  ) -> Result<(Option<CoreDebugTransactions>, Network), ForkReadError> {
    let runtime = self.worker.node.runtime();
    let transactions = match runtime.core_debug_block_source(hash) {
      Some(source) => capture_debug_transactions(source)?,
      None => None,
    };

    Ok((transactions, network(runtime.network_id())))
  }

  pub(crate) fn core_fee_history(
    self,
    count: u64,
    newest: EpochSelector,
    percentiles: RewardPercentiles,
  ) -> Result<FeeHistory, FeeQueryError> {
    self
      .worker
      .node
      .runtime()
      .core_fee_history(count, newest, &percentiles)
  }

  pub(crate) fn core_max_priority_fee_per_gas(self) -> Result<cfx_types::U256, ForkReadError> {
    self.worker.node.runtime().core_max_priority_fee_per_gas()
  }

  pub(crate) fn core_gas_price(self) -> Result<cfx_types::U256, ForkReadError> {
    self.worker.node.runtime().core_gas_price()
  }

  fn checked_native_request(
    &self,
    value: NativeTransactionRequest,
  ) -> Result<(TransactionRequest, Network), UnexpectedRpcAddressNetwork> {
    let (request, supplied_network) = value.into_parts();
    let network = network(self.worker.node.runtime().network_id());
    check_rpc_address_network(supplied_network, &network)?;
    Ok((request, network))
  }

  pub(crate) fn core_accounts(self) -> (Vec<Address>, Network) {
    let runtime = self.worker.node.runtime();
    (
      runtime.signing_accounts(Space::Native),
      network(runtime.network_id()),
    )
  }

  pub(crate) fn core_sign_message(
    self,
    address: RpcAddress,
    message: &[u8],
  ) -> Result<cfxkey::Signature, CoreOperationError<MessageSigningError>> {
    let address = self
      .native_address(address)
      .map_err(CoreOperationError::Network)?;
    let runtime = self.worker.node.runtime();
    let network = network(runtime.network_id());
    runtime
      .sign_message(address.with_native_space(), message)
      .map_err(|error| CoreOperationError::Operation { error, network })
  }

  pub(crate) fn core_send_raw_transaction(
    self,
    raw: &[u8],
  ) -> Result<H256, RuntimeTransactionError> {
    let hash = self
      .worker
      .node
      .runtime_mut()
      .submit_core_raw_transaction(raw)?;
    Ok(self.transaction_admitted(hash))
  }

  pub(crate) fn core_send_transaction(
    self,
    input: NativeTransactionRequest,
  ) -> Result<H256, CoreOperationError<RuntimeTransactionError>> {
    let (request, network) = self
      .checked_native_request(input)
      .map_err(CoreOperationError::Network)?;
    let hash = self
      .worker
      .node
      .runtime_mut()
      .submit_transaction_request(request)
      .map_err(|error| CoreOperationError::Operation { error, network })?;
    Ok(self.transaction_admitted(hash))
  }

  pub(crate) fn core_sign_transaction(
    self,
    input: NativeTransactionRequest,
  ) -> Result<Vec<u8>, CoreOperationError<RuntimeTransactionError>> {
    let (request, network) = self
      .checked_native_request(input)
      .map_err(CoreOperationError::Network)?;
    self
      .worker
      .node
      .runtime()
      .sign_transaction_request(request)
      .map_err(|error| CoreOperationError::Operation { error, network })
  }

  pub(crate) fn core_call(
    self,
    input: NativeTransactionRequest,
    selector: StateSelector,
  ) -> Result<Vec<u8>, CoreOperationError<RuntimeSimulationError>> {
    let (request, network) = self
      .checked_native_request(input)
      .map_err(CoreOperationError::Network)?;
    self
      .worker
      .node
      .runtime()
      .core_call(request, selector)
      .map_err(|error| CoreOperationError::Operation { error, network })
  }

  pub(crate) fn core_estimate(
    self,
    input: NativeTransactionRequest,
    selector: StateSelector,
  ) -> Result<TransactionEstimate, CoreOperationError<RuntimeSimulationError>> {
    let (request, network) = self
      .checked_native_request(input)
      .map_err(CoreOperationError::Network)?;
    self
      .worker
      .node
      .runtime()
      .core_estimate(request, selector)
      .map_err(|error| CoreOperationError::Operation { error, network })
  }
}
