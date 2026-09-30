mod response;

pub(crate) use response::EthereumBlock;

use std::time::Duration;

use alloy_provider::{Provider, RootProvider};
use alloy_rpc_client::{ClientBuilder, RpcClient};
use alloy_transport::{TransportResult, layers::RetryBackoffLayer};

use cfx_rpc_cfx_types::{
  Account, Block, BlockHashOrEpochNumber, Bytes, EpochNumber, Log, PoSEconomics, Receipt,
  RpcAddress, SponsorInfo, Status, StorageCollateralInfo, TokenSupplyInfo, Transaction,
  VoteParamsInfo,
};

use cfx_types::{H256, U64, U256};
use primitives::{DepositInfo, VoteStakeInfo};
use serde::{Deserialize, Serialize};

use response::EthereumReceiptResponse;

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

// Keep protocol method names at the RPC boundary.
#[allow(non_snake_case)]
impl ConfluxRpcClient {
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

  pub(crate) async fn cfx_getBlocksByEpoch(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<H256>> {
    self.core.request("cfx_getBlocksByEpoch", (epoch,)).await
  }

  pub(crate) async fn cfx_getBlockByHashWithPivotAssumption(
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

  pub(crate) async fn cfx_getEpochReceipts(
    &self,
    epoch: BlockHashOrEpochNumber,
    include_espace: bool,
  ) -> TransportResult<Option<Vec<Vec<Receipt>>>> {
    self
      .core
      .request("cfx_getEpochReceipts", (epoch, include_espace))
      .await
  }

  pub(crate) fn espace(&self) -> &RootProvider {
    &self.espace
  }

  /// Returns the block with transaction hashes.
  pub(crate) async fn cfx_getBlockByHash(&self, hash: H256) -> TransportResult<Option<Block>> {
    self.core.request("cfx_getBlockByHash", (hash, false)).await
  }

  /// Returns the block with transaction hashes.
  pub(crate) async fn cfx_getBlockByBlockNumber(
    &self,
    number: U64,
  ) -> TransportResult<Option<Block>> {
    self
      .core
      .request("cfx_getBlockByBlockNumber", (number, false))
      .await
  }

  pub(crate) async fn cfx_getTransactionByHash(
    &self,
    hash: H256,
  ) -> TransportResult<Option<Transaction>> {
    self.core.request("cfx_getTransactionByHash", (hash,)).await
  }

  pub(crate) async fn cfx_getTransactionReceipt(
    &self,
    hash: H256,
  ) -> TransportResult<Option<Receipt>> {
    self
      .core
      .request("cfx_getTransactionReceipt", (hash,))
      .await
  }

  pub(crate) async fn cfx_getLogs(&self, block_hashes: Vec<H256>) -> TransportResult<Vec<Log>> {
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

  pub(crate) async fn eth_getBlockByNumber(
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

  pub(crate) async fn eth_getBlockByHash(
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

  pub(crate) async fn eth_getTransactionByHash(
    &self,
    hash: H256,
  ) -> TransportResult<Option<cfx_rpc_eth_types::Transaction>> {
    self
      .espace
      .client()
      .request("eth_getTransactionByHash", (hash,))
      .await
  }

  pub(crate) async fn eth_getTransactionReceipt(
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

  pub(crate) async fn eth_getLogs(
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

  pub(crate) async fn cfx_getStatus(&self) -> TransportResult<Status> {
    self.core.request_noparams("cfx_getStatus").await
  }

  /// Returns the epoch's pivot block with transaction hashes.
  pub(crate) async fn cfx_getBlockByEpochNumber(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<Option<Block>> {
    self
      .core
      .request("cfx_getBlockByEpochNumber", (epoch, false))
      .await
  }

  pub(crate) async fn cfx_getBalance(
    &self,
    address: RpcAddress,
    block: BlockHashOrEpochNumber,
  ) -> TransportResult<U256> {
    self.core.request("cfx_getBalance", (address, block)).await
  }

  pub(crate) async fn cfx_getAdmin(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Option<RpcAddress>> {
    self.core.request("cfx_getAdmin", (address, epoch)).await
  }

  pub(crate) async fn cfx_getAccount(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Account> {
    self.core.request("cfx_getAccount", (address, epoch)).await
  }

  pub(crate) async fn cfx_getSponsorInfo(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<SponsorInfo> {
    self
      .core
      .request("cfx_getSponsorInfo", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_getCollateralForStorage(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getCollateralForStorage", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_getCode(
    &self,
    address: RpcAddress,
    block: BlockHashOrEpochNumber,
  ) -> TransportResult<Bytes> {
    self.core.request("cfx_getCode", (address, block)).await
  }

  pub(crate) async fn cfx_getStorageAt(
    &self,
    address: RpcAddress,
    slot: U256,
    block: BlockHashOrEpochNumber,
  ) -> TransportResult<Option<H256>> {
    self
      .core
      .request("cfx_getStorageAt", (address, slot, block))
      .await
  }

  pub(crate) async fn cfx_getDepositList(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<DepositInfo>> {
    self
      .core
      .request("cfx_getDepositList", (address, epoch))
      .await
  }

  pub(crate) async fn cfx_getVoteList(
    &self,
    address: RpcAddress,
    epoch: EpochNumber,
  ) -> TransportResult<Vec<VoteStakeInfo>> {
    self.core.request("cfx_getVoteList", (address, epoch)).await
  }

  pub(crate) async fn cfx_getInterestRate(&self, epoch: EpochNumber) -> TransportResult<U256> {
    self.core.request("cfx_getInterestRate", (epoch,)).await
  }

  pub(crate) async fn cfx_getAccumulateInterestRate(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<U256> {
    self
      .core
      .request("cfx_getAccumulateInterestRate", (epoch,))
      .await
  }

  pub(crate) async fn cfx_getSupplyInfo(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<TokenSupplyInfo> {
    self.core.request("cfx_getSupplyInfo", (epoch,)).await
  }

  pub(crate) async fn cfx_getPoSEconomics(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<PoSEconomics> {
    self.core.request("cfx_getPoSEconomics", (epoch,)).await
  }

  pub(crate) async fn cfx_getParamsFromVote(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<VoteParamsInfo> {
    self.core.request("cfx_getParamsFromVote", (epoch,)).await
  }

  pub(crate) async fn cfx_getFeeBurnt(&self, epoch: EpochNumber) -> TransportResult<U256> {
    self.core.request("cfx_getFeeBurnt", (epoch,)).await
  }

  pub(crate) async fn cfx_getCollateralInfo(
    &self,
    epoch: EpochNumber,
  ) -> TransportResult<StorageCollateralInfo> {
    self.core.request("cfx_getCollateralInfo", (epoch,)).await
  }

  pub(crate) async fn pos_getBlockByHash(&self, hash: H256) -> TransportResult<Option<PosBlock>> {
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
