//! Loads the remote starting point for a local fork.

use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockNumberOrTag;
use alloy_transport::TransportError;
use cfx_parameters::{
  RATIO_BASE_TEN,
  block::{CIP1559_CORE_TRANSACTION_GAS_RATIO, CIP1559_ESPACE_TRANSACTION_GAS_RATIO},
};
use cfx_rpc_cfx_types::{Block as CoreBlock, EpochNumber};
use cfx_types::{H256, SpaceMap, U256, U512};
use diem_types::block_info::PivotBlockDecision;
use primitives::{BlockNumber, block::BlockHeight, pos::PosBlockId};
use thiserror::Error;

use crate::rpc_client::ConfluxRpcClient;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct NetworkIdentity {
  pub(crate) network_id: u64,
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
  pub(crate) network: NetworkIdentity,
  pub(crate) epoch_height: BlockHeight,
  pub(crate) pivot_hash: H256,
  /// Core counts all ordered blocks, so this can exceed the epoch height.
  pub(crate) pivot_block_number: BlockNumber,
  pub(crate) timestamp: u64,
  /// The lower bound recovered from RPC, within one gas of the remote header.
  pub(crate) header_gas_limit: U256,
  pub(crate) base_price: Option<SpaceMap<U256>>,
  pub(crate) pos_reference: PosBlockId,
  pub(crate) pos_view: u64,
  pub(crate) pivot_decision: PivotBlockDecision,
}

#[derive(Debug, Error)]
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
}

/// Loads a fixed remote epoch and the parent inputs for local execution.
///
/// The caller selects the local execution rules and checks any expected network
/// identity before starting a runtime.
///
/// Returns an error if required data is unavailable, inconsistent, or unsupported.
/// No runtime is created or modified.
pub(crate) async fn load_fork_base(
  rpc: &ConfluxRpcClient,
  epoch: ForkEpoch,
) -> Result<ForkBase, ForkLoadError> {
  let status = rpc.cfx_getStatus().await?;
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
    network_id: status.network_id.as_u64(),
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
  if epoch_height == BlockHeight::MAX || block_number == BlockNumber::MAX {
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
    network,
    epoch_height,
    pivot_hash: pivot.hash,
    pivot_block_number: block_number,
    timestamp,
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
