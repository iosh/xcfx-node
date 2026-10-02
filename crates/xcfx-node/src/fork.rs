//! Fixed remote baselines and shared reads for local fork execution.

mod cache;
mod client;
mod history;
mod rpc;
mod service;

pub(crate) use cache::ForkCacheConfig;
pub(crate) use client::ForkClient;
pub(crate) use history::{HistoryBlockId, HistoryQuery, HistoryResult};
pub(crate) use rpc::ForkRpc;
pub(crate) use service::ForkReadTask;

use std::{fmt, num::NonZeroU64, sync::Arc};

use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockNumberOrTag;
use alloy_transport::TransportError;
use cfx_parameters::{
  RATIO_BASE_TEN,
  block::{CIP1559_CORE_TRANSACTION_GAS_RATIO, CIP1559_ESPACE_TRANSACTION_GAS_RATIO},
  consensus::ERA_DEFAULT_EPOCH_COUNT,
};
use cfx_rpc_cfx_types::{Block as CoreBlock, EpochNumber, Receipt};
use cfx_types::{Address, H256, SpaceMap, U256, U512};
use diem_types::block_info::PivotBlockDecision;
use primitives::{BlockNumber, block::BlockHeight, pos::PosBlockId};
use thiserror::Error;

use crate::{
  chain_spec::ChainIds,
  rpc_client::ConfluxRpcClient,
  runtime::{Stability, StabilitySource, StablePosition},
};

/// Source identity retained independently of local execution overrides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NetworkIdentity {
  pub(crate) chain_ids: ChainIds,
  pub(crate) genesis_hash: H256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForkEpoch {
  LatestState,
  Number(BlockHeight),
}

/// A remote pivot and the parent inputs needed to extend it locally.
#[derive(Debug)]
pub(crate) struct ForkBase {
  pub(crate) stability: Stability,
  pub(crate) network: NetworkIdentity,
  /// Core epochs per consensus era, used to align checkpoints.
  /// Resolved from the mainnet preset or an explicit override; RPC does not expose it.
  pub(crate) era_epoch_count: NonZeroU64,
  pub(crate) epoch_height: BlockHeight,
  pub(crate) pivot_hash: H256,
  /// Core counts all ordered blocks, so this can exceed the epoch height.
  pub(crate) pivot_block_number: BlockNumber,
  pub(crate) timestamp: u64,
  pub(crate) author: Address,
  /// The lower bound recovered from RPC, within one gas of the remote header.
  pub(crate) header_gas_limit: U256,
  pub(crate) base_price: Option<SpaceMap<U256>>,
  pub(crate) pos_reference: PosBlockId,
  pub(crate) pos_view: u64,
  pub(crate) pivot_decision: PivotBlockDecision,
}

/// Remote epoch receipts in block execution order, tied to the queried pivot.
///
/// `receipts[i]` belongs to `block_hashes[i]`, including blocks with no receipts.
pub(crate) struct ForkEpochReceipts {
  pub(crate) pivot_hash: H256,
  pub(crate) block_hashes: Vec<H256>,
  pub(crate) receipts: Vec<Vec<Receipt>>,
}

/// Display and Debug omit raw RPC errors; the source retains the original cause.
#[derive(Clone, Error)]
pub(crate) enum ForkReadError {
  #[error("remote RPC request failed")]
  Rpc(#[source] Arc<TransportError>),

  #[error("fork read service is closed")]
  Closed,

  #[error("fork read service stopped unexpectedly")]
  ServiceStopped,

  #[error("fork data unavailable: {0}")]
  Unavailable(&'static str),

  #[error("inconsistent fork response: {0}")]
  Inconsistent(&'static str),

  #[error("fork value exceeds the supported range: {0}")]
  OutOfRange(&'static str),

  #[error("unsupported fork read: {0}")]
  Unsupported(&'static str),
}

impl From<TransportError> for ForkReadError {
  fn from(error: TransportError) -> Self {
    Self::Rpc(Arc::new(error))
  }
}

impl fmt::Debug for ForkReadError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Display and Debug omit raw RPC errors; the source retains the original cause.
#[derive(Error)]
pub(crate) enum ForkLoadError {
  #[error("remote RPC request failed")]
  Rpc(#[from] TransportError),

  #[error("fork data unavailable: {0}")]
  Unavailable(String),

  #[error("required RPC field is missing: {0}")]
  MissingField(&'static str),

  #[error("RPC field {0} exceeds the supported integer range")]
  OutOfRange(&'static str),

  #[error("invalid fork base: {0}")]
  InvalidBase(String),

  #[error("unsupported fork base: {0}")]
  Unsupported(&'static str),

  #[error("fork.era_epoch_count is required for source network {network_id}, which has no preset")]
  MissingEraEpochCount { network_id: u64 },
}

impl fmt::Debug for ForkLoadError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Loads a fixed remote epoch and the parent inputs for local execution.
///
/// `LatestState` is resolved once. Core and eSpace must identify the same pivot.
///
/// # Errors
/// Returns an error if required data is unavailable, inconsistent, or unsupported.
///
/// # Panics
/// Panics if the remote chain IDs violate Conflux's `u32` range.
pub(crate) async fn load_fork_base(
  rpc: &ConfluxRpcClient,
  epoch: ForkEpoch,
  era_epoch_count: Option<NonZeroU64>,
) -> Result<ForkBase, ForkLoadError> {
  let status = rpc.cfx_getStatus().await?;
  // Conflux exposes its u32 chain IDs as U64 RPC quantities.
  let chain_ids = ChainIds {
    chain_id: status.chain_id.as_u32(),
    espace_chain_id: status.ethereum_space_chain_id.as_u32(),
    network_id: status.network_id.as_u64(),
  };
  let era_epoch_count = match (era_epoch_count, chain_ids.network_id) {
    (Some(count), _) => count,
    (None, 1029) => {
      NonZeroU64::new(ERA_DEFAULT_EPOCH_COUNT).expect("the mainnet era preset must be nonzero")
    }
    (None, network_id) => return Err(ForkLoadError::MissingEraEpochCount { network_id }),
  };
  // Keep the selected epoch fixed while the remote chain advances.
  let latest_state = status.latest_state.as_u64();
  let epoch_height = match epoch {
    ForkEpoch::LatestState => latest_state,
    ForkEpoch::Number(height) => height,
  };
  if epoch_height > latest_state {
    return Err(ForkLoadError::Unavailable(format!(
      "epoch {epoch_height} is beyond latest_state {latest_state}",
    )));
  }

  let genesis = load_pivot(rpc, 0).await?;
  let network = NetworkIdentity {
    chain_ids,
    genesis_hash: genesis.hash,
  };
  let pivot = if epoch_height == 0 {
    genesis
  } else {
    load_pivot(rpc, epoch_height).await?
  };
  let block_number = pivot
    .block_number
    .ok_or(ForkLoadError::MissingField("Core blockNumber"))?;
  let block_number = BlockNumber::try_from(block_number)
    .map_err(|_| ForkLoadError::OutOfRange("Core blockNumber"))?;
  // The first local epoch and block each advance by one.
  // BlockReceipts preserves Conflux's historical extra increment, so the
  // first local execution needs room for two block-number increments.
  if epoch_height == BlockHeight::MAX || block_number.checked_add(2).is_none() {
    return Err(ForkLoadError::Unsupported(
      "fork epoch or block number cannot advance",
    ));
  }
  let timestamp =
    u64::try_from(pivot.timestamp).map_err(|_| ForkLoadError::OutOfRange("Core timestamp"))?;

  let espace = rpc
    .espace()
    .get_block_by_number(BlockNumberOrTag::Number(epoch_height))
    .await?
    .ok_or_else(|| ForkLoadError::Unavailable(format!("eSpace block at epoch {epoch_height}")))?
    .header;
  let espace_hash = H256::from(espace.hash.0);
  if espace_hash != pivot.hash {
    return Err(ForkLoadError::InvalidBase(format!(
      "Core/eSpace pivot mismatch at epoch {epoch_height}: {:?} / {espace_hash:?}",
      pivot.hash,
    )));
  }

  let base_price = match (pivot.base_fee_per_gas, espace.base_fee_per_gas) {
    (Some(core), Some(espace)) => Some(SpaceMap::new(core, U256::from(espace))),
    (None, None) => None,
    _ => {
      return Err(ForkLoadError::InvalidBase(
        "Core and eSpace disagree on baseFeePerGas presence".into(),
      ));
    }
  };
  let header_gas_limit = header_gas_limit_from_rpc(
    pivot.gas_limit,
    U256::from(espace.gas_limit),
    base_price.is_some(),
  )?;

  let pos_reference = pivot.pos_reference.ok_or(ForkLoadError::Unsupported(
    "local extension requires a fixed PoS reference",
  ))?;
  let (pos_view, pivot_decision) = load_pos_metadata(rpc, pos_reference).await?;

  // blockNumber depends on epoch ordering and is not part of the header hash.
  let current = load_pivot(rpc, epoch_height).await?;
  if current.hash != pivot.hash || current.block_number != pivot.block_number {
    return Err(ForkLoadError::InvalidBase(format!(
      "fork pivot or block number changed while loading epoch {epoch_height}",
    )));
  }

  Ok(ForkBase {
    stability: Stability {
      confirmed: StablePosition {
        height: status.latest_confirmed.as_u64().min(epoch_height),
        source: StabilitySource::Fork,
      },
      finalized: StablePosition {
        height: status.latest_finalized.as_u64().min(epoch_height),
        source: StabilitySource::Fork,
      },
      checkpoint: StablePosition {
        height: status.latest_checkpoint.as_u64().min(epoch_height),
        source: StabilitySource::Fork,
      },
    },
    network,
    era_epoch_count,
    epoch_height,
    pivot_hash: pivot.hash,
    pivot_block_number: block_number,
    timestamp,
    author: pivot.miner.hex_address,
    header_gas_limit,
    base_price,
    pos_reference,
    pos_view,
    pivot_decision,
  })
}

async fn load_pos_metadata(
  rpc: &ConfluxRpcClient,
  reference: PosBlockId,
) -> Result<(u64, PivotBlockDecision), ForkLoadError> {
  let pos = rpc
    .pos_getBlockByHash(reference)
    .await?
    .ok_or_else(|| ForkLoadError::Unavailable(format!("PoS block for reference {reference:?}")))?;
  let pivot_decision = pos
    .pivot_decision
    .ok_or(ForkLoadError::MissingField("PoS pivotDecision"))?;

  Ok((
    pos.height.as_u64(),
    PivotBlockDecision {
      block_hash: pivot_decision.block_hash,
      height: pivot_decision.height.as_u64(),
    },
  ))
}

async fn load_pivot(
  rpc: &ConfluxRpcClient,
  epoch_height: BlockHeight,
) -> Result<CoreBlock, ForkLoadError> {
  rpc
    .cfx_getBlockByEpochNumber(EpochNumber::Num(epoch_height.into()))
    .await?
    .ok_or_else(|| ForkLoadError::Unavailable(format!("Core pivot at epoch {epoch_height}")))
}

fn header_gas_limit_from_rpc(
  core_gas_limit: U256,
  espace_gas_limit: U256,
  has_base_fee: bool,
) -> Result<U256, ForkLoadError> {
  let ratio_base = U512::from(RATIO_BASE_TEN);
  let header_gas_limit = if has_base_fee {
    // Core RPC reports floor(9 * header_limit / 10); use the interval's lower bound.
    let divisor = U512::from(CIP1559_CORE_TRANSACTION_GAS_RATIO);
    let numerator = U512::from(core_gas_limit) * ratio_base;
    let lower_bound = (numerator + divisor - U512::one()) / divisor;
    U256::try_from(lower_bound).map_err(|_| ForkLoadError::OutOfRange("Core gasLimit"))?
  } else {
    core_gas_limit
  };

  let espace_projection =
    U512::from(header_gas_limit) * U512::from(CIP1559_ESPACE_TRANSACTION_GAS_RATIO) / ratio_base;
  if header_gas_limit.is_zero() {
    return Err(ForkLoadError::InvalidBase(
      "Core gasLimit must be nonzero".into(),
    ));
  }
  if espace_projection != U512::from(espace_gas_limit) {
    return Err(ForkLoadError::InvalidBase(format!(
      "eSpace gasLimit mismatch: expected {espace_projection}, received {espace_gas_limit}",
    )));
  }

  Ok(header_gas_limit)
}
