use alloy_provider::RootProvider;
use alloy_rpc_client::RpcClient;
use alloy_transport::TransportResult;
use cfx_rpc_cfx_types::{Block, EpochNumber, Status};
use cfx_types::{H256, U64};
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
