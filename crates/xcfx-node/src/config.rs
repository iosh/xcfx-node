//! Typed startup input shared by embedded and network entry points.

use std::{num::NonZeroU64, time::Duration};

use cfx_types::U256;
use reqwest::Url;
use thiserror::Error;

use crate::{
  fork::{ForkCacheConfig, ForkEpoch},
  rpc_client::HttpRpcConfig,
  signing::AccountSource,
};

/// The local execution profile, independent of a fork's source network identity.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ProtocolProfile {
  #[default]
  Development,
}

/// Scheduling uses wall time; block timestamps use `timestamp_increment`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MiningMode {
  #[default]
  Auto,
  Manual,
  Interval(Duration),
}

/// Chain and address-network identifiers for a source or local instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChainIdentity {
  pub(crate) chain_id: u32,
  pub(crate) espace_chain_id: u32,
  pub(crate) network_id: u64,
}

impl Default for ChainIdentity {
  fn default() -> Self {
    let chain_id = 201_029;
    Self {
      chain_id,
      espace_chain_id: 31_337,
      network_id: u64::from(chain_id),
    }
  }
}

/// Signing sources and the initial balance assigned to each address in each Space.
/// Secret-bearing configuration deliberately has no `Debug` or serialization impl.
pub(crate) struct AccountConfig {
  pub(crate) source: AccountSource,
  pub(crate) balance: U256,
}

impl Default for AccountConfig {
  fn default() -> Self {
    Self {
      source: AccountSource::default(),
      balance: U256::from(10_000) * U256::exp10(18),
    }
  }
}

/// Parsed remote endpoints and policies. Formatting must not expose URL credentials.
pub(crate) struct ForkOptions {
  pub(crate) core_url: Url,
  pub(crate) espace_url: Url,
  pub(crate) epoch: ForkEpoch,
  pub(crate) http: HttpRpcConfig,
  pub(crate) cache: ForkCacheConfig,
  /// `None` defers to the source network's preset, resolved during fork startup.
  pub(crate) era_epoch_count: Option<NonZeroU64>,
}

impl ForkOptions {
  pub(crate) fn new(core_url: Url, espace_url: Url) -> Self {
    Self {
      core_url,
      espace_url,
      epoch: ForkEpoch::LatestState,
      http: HttpRpcConfig::default(),
      cache: ForkCacheConfig::default(),
      era_epoch_count: None,
    }
  }

  fn validate(&self) -> Result<(), ConfigError> {
    for (field, url) in [
      ("core_url", &self.core_url),
      ("espace_url", &self.espace_url),
    ] {
      if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ConfigError::InvalidForkEndpoint { field });
      }
    }
    Ok(())
  }
}

/// Parsed instance configuration. Transport listeners have their own configuration.
/// Chain IDs remain optional so startup can distinguish overrides from defaults.
pub(crate) struct NodeConfig {
  pub(crate) profile: ProtocolProfile,
  pub(crate) chain_id: Option<u32>,
  pub(crate) espace_chain_id: Option<u32>,
  pub(crate) network_id: Option<u64>,
  pub(crate) accounts: AccountConfig,
  pub(crate) mining: MiningMode,
  pub(crate) timestamp_increment: u64,
  pub(crate) confirmed_depth: u64,
  pub(crate) finalized_depth: u64,
  pub(crate) fork: Option<ForkOptions>,
  pub(crate) max_call_gas: u64,
  pub(crate) max_transactions: usize,
  pub(crate) max_state_controls: usize,
  pub(crate) filter_idle_timeout: Duration,
  pub(crate) logging: bool,
}

impl Default for NodeConfig {
  fn default() -> Self {
    Self {
      profile: ProtocolProfile::default(),
      chain_id: None,
      espace_chain_id: None,
      network_id: None,
      accounts: AccountConfig::default(),
      mining: MiningMode::default(),
      timestamp_increment: 1,
      confirmed_depth: 32,
      finalized_depth: 64,
      fork: None,
      max_call_gas: 30_000_000,
      max_transactions: 4_096,
      max_state_controls: 4_096,
      filter_idle_timeout: Duration::from_secs(300),
      logging: false,
    }
  }
}

impl NodeConfig {
  /// Checks local constraints before allocating network or execution resources.
  /// Fork identity and state-dependent funding checks follow remote loading.
  pub(crate) fn validate(&self) -> Result<(), ConfigError> {
    if matches!(self.mining, MiningMode::Interval(interval) if interval.is_zero()) {
      return Err(ConfigError::ZeroMiningInterval);
    }
    if let Some(fork) = &self.fork {
      fork.validate()?;
    }
    Ok(())
  }

  /// Local startup defaults the network ID to the effective Core chain ID.
  pub(crate) fn local_chain_identity(&self) -> ChainIdentity {
    assert!(self.fork.is_none(), "local identity requires local startup");
    let defaults = ChainIdentity::default();
    let chain_id = self.chain_id.unwrap_or(defaults.chain_id);
    ChainIdentity {
      chain_id,
      espace_chain_id: self.espace_chain_id.unwrap_or(defaults.espace_chain_id),
      network_id: self.network_id.unwrap_or(u64::from(chain_id)),
    }
  }

  /// Fork startup requires the loaded source identity before applying overrides.
  pub(crate) fn fork_chain_identity(&self, source: ChainIdentity) -> ChainIdentity {
    assert!(self.fork.is_some(), "fork identity requires fork startup");
    ChainIdentity {
      chain_id: self.chain_id.unwrap_or(source.chain_id),
      espace_chain_id: self.espace_chain_id.unwrap_or(source.espace_chain_id),
      network_id: self.network_id.unwrap_or(source.network_id),
    }
  }
}

/// Startup configuration failures, independent of any RPC or N-API error format.
#[derive(Debug, Error)]
pub(crate) enum ConfigError {
  #[error("mining interval must be greater than zero")]
  ZeroMiningInterval,
  #[error("fork {field} must be an HTTP or HTTPS endpoint with a host")]
  InvalidForkEndpoint { field: &'static str },
}
