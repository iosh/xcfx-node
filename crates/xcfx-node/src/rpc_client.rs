use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::TransportResult;
use cfx_rpc_cfx_types::{
  Account, Block, BlockHashOrEpochNumber, Bytes, EpochNumber, PoSEconomics, RpcAddress,
  SponsorInfo, Status, StorageCollateralInfo, TokenSupplyInfo, VoteParamsInfo,
};
use cfx_types::{H256, U64, U256};
use primitives::{DepositInfo, VoteStakeInfo};
use serde::Deserialize;

/// RPC connections for Conflux Core Space, PoS, and eSpace.
#[derive(Clone)]
pub(crate) struct ConfluxRpcClient {
  core: RpcClient,
  espace: RootProvider,
}

// Keep protocol method names at the RPC boundary.
#[allow(non_snake_case)]
impl ConfluxRpcClient {
  pub(crate) fn new(core: RpcClient, espace: RpcClient) -> Self {
    Self {
      core,
      espace: RootProvider::new(espace),
    }
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

  pub(crate) fn espace(&self) -> &RootProvider {
    &self.espace
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
