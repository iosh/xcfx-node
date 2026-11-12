mod response;

pub(crate) use response::{CoreFeeHistoryResponse, EthereumBlock, EthereumFeeHistoryResponse};

use std::time::Duration;

use alloy_provider::{Provider, RootProvider};
use alloy_rpc_client::{ClientBuilder, RpcClient};
use alloy_transport::{TransportResult, layers::RetryBackoffLayer};

use cfx_rpc_cfx_types::{
  Account, Block, Bytes, EpochNumber, Log, PoSEconomics, Receipt, RpcAddress, SponsorInfo, Status,
  StorageCollateralInfo, TokenSupplyInfo, Transaction, TransactionRequest, VoteParamsInfo,
};

use cfx_types::{Address, H256, U64, U256};
use primitives::{DepositInfo, VoteStakeInfo};
use serde::{Deserialize, Serialize};

use response::EthereumReceiptResponse;

/// A selected Core pivot in the standard object form accepted by state RPCs.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PivotSelector {
  block_hash: H256,
  require_pivot: bool,
}

impl PivotSelector {
  fn new(block_hash: H256) -> Self {
    Self {
      block_hash,
      require_pivot: true,
    }
  }
}

/// HTTP timeout and retry policy shared by the Core and eSpace endpoints.
#[derive(Debug)]
pub(crate) struct HttpRpcConfig {
  /// Timeout for one HTTP attempt, including its complete response body.
  /// Retry delays and subsequent attempts are outside this timeout.
  pub(crate) request_timeout: Duration,
  /// Retries after the initial attempt, for errors Alloy marks as retryable.
  /// Zero disables retries.
  pub(crate) max_retries: u32,
  /// Fallback delay between retries in milliseconds; remote hints take precedence.
  pub(crate) retry_backoff_ms: u64,
}

impl Default for HttpRpcConfig {
  fn default() -> Self {
    Self {
      request_timeout: Duration::from_secs(45),
      max_retries: 5,
      retry_backoff_ms: 1_000,
    }
  }
}

/// RPC connections for Conflux Core Space, PoS, and eSpace.
#[derive(Clone)]
pub(crate) struct ConfluxRpcClient {
  core: RpcClient,
  espace: RootProvider,
}

impl ConfluxRpcClient {
  pub(crate) async fn debug_get_transactions_by_block(
    &self,
    hash: H256,
  ) -> TransportResult<Vec<cfx_rpc_eth_types::WrapTransaction>> {
    let transactions: Vec<response::CoreDebugTransactionResponse> = self
      .core
      .request("debug_getTransactionsByBlock", (hash,))
      .await?;

    Ok(
      transactions
        .into_iter()
        .map(|transaction| transaction.0)
        .collect(),
    )
  }

  pub(crate) async fn cfx_fee_history(
    &self,
    block_count: U64,
    newest_epoch: EpochNumber,
    reward_percentiles: &[f64],
  ) -> TransportResult<CoreFeeHistoryResponse> {
    self
      .core
      .request(
        "cfx_feeHistory",
        (block_count, newest_epoch, reward_percentiles),
      )
      .await
  }

  pub(crate) async fn cfx_get_staking_balance(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getStakingBalance", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_check_balance_against_transaction(
    &self,
    account: RpcAddress,
    contract: RpcAddress,
    gas_limit: U256,
    gas_price: U256,
    storage_limit: U256,
    epoch: EpochNumber,
  ) -> TransportResult<cfx_rpc_cfx_types::CheckBalanceAgainstTransactionResponse> {
    self
      .core
      .request(
        "cfx_checkBalanceAgainstTransaction",
        (
          account,
          contract,
          gas_limit,
          gas_price,
          storage_limit,
          epoch,
        ),
      )
      .await
  }

  /// Uses caller-configured transports, including their timeout and retry policy.
  pub(crate) fn new(core: RpcClient, espace: RpcClient) -> Self {
    Self {
      core,
      espace: RootProvider::new(espace),
    }
  }

  /// Builds HTTP clients for both endpoints with the supplied timeout and retry policy.
  /// No RPC requests are sent during construction.
  ///
  /// # Errors
  /// Returns the underlying error if the HTTP client cannot be built.
  pub(crate) fn http(
    core: reqwest::Url,
    espace: reqwest::Url,
    config: &HttpRpcConfig,
  ) -> Result<Self, reqwest::Error> {
    let http = reqwest::Client::builder()
      .timeout(config.request_timeout)
      // Alloy owns retries; disable reqwest's independent retry policy.
      .retry(reqwest::retry::never())
      .build()?;
    let rpc = |url| {
      let builder = ClientBuilder::default();
      if config.max_retries == 0 {
        builder.http_with_client(http.clone(), url)
      } else {
        builder
          .layer(RetryBackoffLayer::new(
            config.max_retries,
            config.retry_backoff_ms,
            // Do not add a provider-specific compute-unit delay.
            u64::MAX,
          ))
          .http_with_client(http.clone(), url)
      }
    };
    Ok(Self::new(rpc(core), rpc(espace)))
  }

  pub(crate) async fn cfx_get_blocks_by_epoch(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<H256>> {
    self.core.request("cfx_getBlocksByEpoch", (epoch,)).await
  }

  pub(crate) async fn cfx_get_skipped_blocks_by_epoch(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<H256>> {
    self
      .core
      .request("cfx_getSkippedBlocksByEpoch", (epoch,))
      .await
  }

  pub(crate) async fn cfx_get_block_by_hash_with_pivot_assumption(
    &self,
    hash: H256,
    pivot: H256,
    epoch: U64,
  ) -> TransportResult<Block> {
    self
      .core
      .request(
        "cfx_getBlockByHashWithPivotAssumption",
        (hash, pivot, epoch),
      )
      .await
  }

  pub(crate) async fn cfx_get_epoch_receipts(
    &self,
    pivot_hash: H256,
    include_espace: bool,
  ) -> TransportResult<Option<Vec<Vec<Receipt>>>> {
    self
      .core
      .request(
        "cfx_getEpochReceipts",
        (PivotSelector::new(pivot_hash), include_espace),
      )
      .await
  }

  pub(crate) fn espace(&self) -> &RootProvider {
    &self.espace
  }

  /// Returns the block with transaction hashes.
  pub(crate) async fn cfx_get_block_by_hash(&self, hash: H256) -> TransportResult<Option<Block>> {
    self.core.request("cfx_getBlockByHash", (hash, false)).await
  }

  /// Returns the block with transaction hashes.
  pub(crate) async fn cfx_get_block_by_block_number(
    &self,
    number: U64,
  ) -> TransportResult<Option<Block>> {
    self
      .core
      .request("cfx_getBlockByBlockNumber", (number, false))
      .await
  }

  pub(crate) async fn cfx_get_transaction_by_hash(
    &self,
    hash: H256,
  ) -> TransportResult<Option<Transaction>> {
    self.core.request("cfx_getTransactionByHash", (hash,)).await
  }

  pub(crate) async fn cfx_get_transaction_receipt(
    &self,
    hash: H256,
  ) -> TransportResult<Option<Receipt>> {
    self
      .core
      .request("cfx_getTransactionReceipt", (hash,))
      .await
  }

  pub(crate) async fn cfx_get_logs(&self, block_hashes: Vec<H256>) -> TransportResult<Vec<Log>> {
    #[derive(Clone, Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct BlockHashFilter {
      block_hashes: Vec<H256>,
    }

    self
      .core
      .request("cfx_getLogs", (BlockHashFilter { block_hashes },))
      .await
  }

  pub(crate) async fn eth_get_block_by_number(
    &self,
    number: U64,
    full_transactions: bool,
  ) -> TransportResult<Option<EthereumBlock>> {
    self
      .espace
      .client()
      .request("eth_getBlockByNumber", (number, full_transactions))
      .await
  }

  pub(crate) async fn eth_get_balance(
    &self,
    address: Address,
    block: cfx_rpc_eth_types::BlockId,
  ) -> TransportResult<U256> {
    self
      .espace
      .client()
      .request("eth_getBalance", (address, block))
      .await
  }

  pub(crate) async fn eth_fee_history(
    &self,
    block_count: U64,
    newest_block: cfx_rpc_eth_types::BlockId,
    reward_percentiles: &[f64],
  ) -> TransportResult<EthereumFeeHistoryResponse> {
    self
      .espace
      .client()
      .request(
        "eth_feeHistory",
        (block_count, newest_block, reward_percentiles),
      )
      .await
  }

  pub(crate) async fn eth_get_transaction_count(
    &self,
    address: Address,
    block: cfx_rpc_eth_types::BlockId,
  ) -> TransportResult<U256> {
    self
      .espace
      .client()
      .request("eth_getTransactionCount", (address, block))
      .await
  }

  pub(crate) async fn eth_get_code(
    &self,
    address: Address,
    block: cfx_rpc_eth_types::BlockId,
  ) -> TransportResult<cfx_rpc_eth_types::Bytes> {
    self
      .espace
      .client()
      .request("eth_getCode", (address, block))
      .await
  }

  pub(crate) async fn eth_get_storage_at(
    &self,
    address: Address,
    position: U256,
    block: cfx_rpc_eth_types::BlockId,
  ) -> TransportResult<H256> {
    self
      .espace
      .client()
      .request("eth_getStorageAt", (address, position, block))
      .await
  }

  pub(crate) async fn eth_get_block_receipts(
    &self,
    block: cfx_rpc_eth_types::BlockId,
  ) -> TransportResult<Option<Vec<cfx_rpc_eth_types::Receipt>>> {
    let receipts: Option<Vec<EthereumReceiptResponse>> = self
      .espace
      .client()
      .request("eth_getBlockReceipts", (block,))
      .await?;
    Ok(receipts.map(|receipts| receipts.into_iter().map(|receipt| receipt.0).collect()))
  }

  pub(crate) async fn eth_get_block_by_hash(
    &self,
    hash: H256,
    full_transactions: bool,
  ) -> TransportResult<Option<EthereumBlock>> {
    self
      .espace
      .client()
      .request("eth_getBlockByHash", (hash, full_transactions))
      .await
  }

  pub(crate) async fn eth_get_transaction_by_block_hash_and_index(
    &self,
    hash: H256,
    index: U256,
  ) -> TransportResult<Option<cfx_rpc_eth_types::Transaction>> {
    self
      .espace
      .client()
      .request("eth_getTransactionByBlockHashAndIndex", (hash, index))
      .await
  }

  pub(crate) async fn eth_get_transaction_by_hash(
    &self,
    hash: H256,
  ) -> TransportResult<Option<cfx_rpc_eth_types::Transaction>> {
    self
      .espace
      .client()
      .request("eth_getTransactionByHash", (hash,))
      .await
  }

  pub(crate) async fn eth_get_transaction_receipt(
    &self,
    hash: H256,
  ) -> TransportResult<Option<cfx_rpc_eth_types::Receipt>> {
    let receipt: Option<EthereumReceiptResponse> = self
      .espace
      .client()
      .request("eth_getTransactionReceipt", (hash,))
      .await?;

    Ok(receipt.map(|response| response.0))
  }

  pub(crate) async fn eth_get_logs(
    &self,
    block_hash: H256,
  ) -> TransportResult<Vec<cfx_rpc_eth_types::Log>> {
    #[derive(Clone, Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct BlockHashFilter {
      block_hash: H256,
    }

    self
      .espace
      .client()
      .request("eth_getLogs", (BlockHashFilter { block_hash },))
      .await
  }

  pub(crate) async fn cfx_get_status(&self) -> TransportResult<Status> {
    self.core.request_noparams("cfx_getStatus").await
  }

  /// Executes against the selected remote state without publishing changes.
  pub(crate) async fn cfx_call(
    &self,
    request: &TransactionRequest,
    pivot_hash: H256,
  ) -> TransportResult<Bytes> {
    self
      .core
      .request("cfx_call", (request, PivotSelector::new(pivot_hash)))
      .await
  }

  /// Returns the epoch's pivot block with transaction hashes.
  pub(crate) async fn cfx_get_block_by_epoch_number(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<Option<Block>> {
    self
      .core
      .request("cfx_getBlockByEpochNumber", (epoch, false))
      .await
  }

  pub(crate) async fn cfx_get_balance(
    &self,
    address: RpcAddress,
    pivot_hash: H256,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getBalance", (address, PivotSelector::new(pivot_hash)))
      .await
  }

  pub(crate) async fn cfx_get_admin(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Option<RpcAddress>> {
    self.core.request("cfx_getAdmin", (address, epoch)).await
  }

  pub(crate) async fn cfx_get_next_nonce(
    &self,
    address: RpcAddress,
    pivot_hash: H256,
  ) -> TransportResult<U256> {
    self
      .core
      .request(
        "cfx_getNextNonce",
        (address, PivotSelector::new(pivot_hash)),
      )
      .await
  }

  pub(crate) async fn cfx_get_account(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Account> {
    self.core.request("cfx_getAccount", (address, epoch)).await
  }

  pub(crate) async fn cfx_get_sponsor_info(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<SponsorInfo> {
    self
      .core
      .request("cfx_getSponsorInfo", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_get_collateral_for_storage(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getCollateralForStorage", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_get_code(
    &self,
    address: RpcAddress,
    pivot_hash: H256,
  ) -> TransportResult<Bytes> {
    self
      .core
      .request("cfx_getCode", (address, PivotSelector::new(pivot_hash)))
      .await
  }

  pub(crate) async fn cfx_get_storage_at(
    &self,
    address: RpcAddress,
    slot: U256,
    pivot_hash: H256,
  ) -> TransportResult<Option<H256>> {
    self
      .core
      .request(
        "cfx_getStorageAt",
        (address, slot, PivotSelector::new(pivot_hash)),
      )
      .await
  }

  pub(crate) async fn cfx_get_deposit_list(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<DepositInfo>> {
    self
      .core
      .request("cfx_getDepositList", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_get_vote_list(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<VoteStakeInfo>> {
    self.core.request("cfx_getVoteList", (address, epoch)).await
  }

  pub(crate) async fn cfx_get_interest_rate(&self, epoch: EpochNumber) -> TransportResult<U256> {
    self.core.request("cfx_getInterestRate", (epoch,)).await
  }

  pub(crate) async fn cfx_get_accumulate_interest_rate(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getAccumulateInterestRate", (epoch,))
      .await
  }

  pub(crate) async fn cfx_get_supply_info(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<TokenSupplyInfo> {
    self.core.request("cfx_getSupplyInfo", (epoch,)).await
  }

  pub(crate) async fn cfx_get_pos_economics(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<PoSEconomics> {
    self.core.request("cfx_getPoSEconomics", (epoch,)).await
  }

  pub(crate) async fn cfx_get_params_from_vote(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<VoteParamsInfo> {
    self.core.request("cfx_getParamsFromVote", (epoch,)).await
  }

  pub(crate) async fn cfx_get_fee_burnt(&self, epoch: EpochNumber) -> TransportResult<U256> {
    self.core.request("cfx_getFeeBurnt", (epoch,)).await
  }

  pub(crate) async fn cfx_get_collateral_info(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<StorageCollateralInfo> {
    self.core.request("cfx_getCollateralInfo", (epoch,)).await
  }

  pub(crate) async fn pos_get_block_by_hash(
    &self,
    hash: H256,
  ) -> TransportResult<Option<PosBlock>> {
    self.core.request("pos_getBlockByHash", (hash,)).await
  }
}

// The upstream PoS RPC types only implement serialization.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PosBlock {
  /// PoS consensus view, independent of Core epoch height.
  pub(crate) height: U64,
  pub(crate) pivot_decision: Option<PosPivotDecision>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PosPivotDecision {
  pub(crate) block_hash: H256,
  pub(crate) height: U64,
}
