//! Startup, Runtime ownership, and shutdown for local and fork nodes.

use std::{collections::BTreeMap, fmt, mem, sync::Arc};

use cfx_parameters::genesis::GENESIS_ACCOUNT_ADDRESS;
use cfx_types::{AddressSpaceUtil, AddressWithSpace, Space, U256};
use thiserror::Error;
use tokio::{
  runtime::Handle,
  task::{JoinError, JoinHandle},
};

use crate::{
  chain_spec::{ChainIds, ChainSpec},
  config::{ConfigError, NodeConfig},
  fork::{ForkBase, ForkCacheConfig, ForkLoadError, ForkReadError, ForkReadTask, ForkRpc},
  genesis::{GenesisError, GenesisHeaderInput},
  pos::GenesisPosDefinition,
  production_environment::{ProductionDefaults, ProductionTimeError},
  rpc_client::ConfluxRpcClient,
  runtime::{NodeRuntime, RuntimeConfig},
  signing::AccountConfigError,
  state::balance::BalanceChangeError,
  transaction_pool::TransactionPoolPolicy,
};

/// Startup stages with safe public text. Sources retain lower-level diagnostics.
#[derive(Error)]
pub(crate) enum NodeStartError {
  #[error("configuration: {0}")]
  Configuration(#[from] ConfigError),
  #[error("configuration accounts: {0}")]
  Accounts(#[from] AccountConfigError),
  #[error("{0}")]
  Genesis(#[from] GenesisError),
  #[error("fork HTTP client construction failed")]
  HttpClient(#[source] reqwest::Error),
  #[error("fork loading: {0}")]
  ForkLoad(#[from] ForkLoadError),
  #[error("fork initial global parameters: {0}")]
  ForkInitialState(#[from] ForkReadError),
  #[error(
    "fork initial balances (accounts.balance): {source}{cleanup}",
    cleanup = if .cleanup_error.is_some() {
      "; fork read service cleanup also failed"
    } else {
      ""
    }
  )]
  ForkFunding {
    #[source]
    source: BalanceChangeError,
    /// Cleanup is always awaited; retain its failure alongside the funding error.
    cleanup_error: Option<JoinError>,
  },
  #[error("fork timestamp_increment: {0}")]
  ForkTimestamp(#[from] ProductionTimeError),
  #[error("unsupported fork execution configuration: {0}")]
  UnsupportedConfiguration(&'static str),
}

impl fmt::Debug for NodeStartError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Display and Debug omit task panic payloads; sources retain join failures.
#[derive(Clone, Error)]
pub(crate) enum NodeError {
  #[error("node is closing or closed")]
  Closed,
  /// Runtime access detects termination synchronously; `close` can attach a join failure.
  #[error("fork read service stopped unexpectedly")]
  ForkServiceStopped(#[source] Option<Arc<JoinError>>),
  #[error("runtime disposal task did not complete normally")]
  RuntimeDisposalFailed(#[source] Arc<JoinError>),
}

impl fmt::Debug for NodeError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Owns one Runtime and, in fork mode, its remote read service.
///
/// Runtime calls require a thread that allows blocking. Keep the host Tokio
/// runtime active until `close` finishes reclaiming the node's resources.
pub(crate) struct Node {
  io: Handle,
  state: NodeState,
}

enum NodeState {
  Running {
    runtime: NodeRuntime,
    read_task: Option<ForkReadTask>,
  },
  Stopping {
    read_task: Option<ForkReadTask>,
    disposal: JoinHandle<()>,
    result: Result<(), NodeError>,
  },
  Closed(Result<(), NodeError>),
}

impl Node {
  /// Resolves typed configuration and starts a local or fork Runtime.
  ///
  /// Runs synchronously on a thread that allows blocking, outside async tasks.
  /// The host Tokio runtime must remain active to serve remote reads and cleanup.
  ///
  /// # Errors
  /// Returns a stage-specific error without exposing a partially initialized node.
  /// If initial funding fails, the fork read service is closed before returning.
  ///
  /// # Panics
  /// Fork startup panics if called inside an async task.
  pub(crate) fn start(io: Handle, config: NodeConfig) -> Result<Self, NodeStartError> {
    config.validate()?;
    let balance = config.accounts.balance;
    let signing_keys = config.accounts.source.build_signing_keys()?;
    let allocations = [Space::Native, Space::Ethereum]
      .into_iter()
      .flat_map(|space| {
        signing_keys
          .addresses(space)
          .into_iter()
          .map(move |address| (address.with_space(space), balance))
      })
      .collect();

    let make_runtime_config = |chain_ids| RuntimeConfig {
      chain_spec: Arc::new(ChainSpec::new(chain_ids)),
      signing_keys,
      production_defaults: ProductionDefaults::new(config.timestamp_increment),
      transaction_pool_policy: TransactionPoolPolicy::new(config.max_transactions),
      max_state_controls: config.max_state_controls,
      confirmed_depth: config.confirmed_depth,
      finalized_depth: config.finalized_depth,
    };

    match config.fork {
      None => {
        let defaults = ChainIds::default();
        let chain_id = config.chain_id.unwrap_or(defaults.chain_id);
        let chain_ids = ChainIds {
          chain_id,
          espace_chain_id: config.espace_chain_id.unwrap_or(defaults.espace_chain_id),
          network_id: config.network_id.unwrap_or(u64::from(chain_id)),
        };
        Self::from_genesis(
          io,
          make_runtime_config(chain_ids),
          allocations,
          GenesisPosDefinition::default(),
        )
      }
      Some(fork) => {
        let client = ConfluxRpcClient::http(fork.core_url, fork.espace_url, &fork.http)
          .map_err(NodeStartError::HttpClient)?;
        let rpc = io.block_on(ForkRpc::connect(client, fork.epoch, fork.era_epoch_count))?;
        let source = rpc.base().network.chain_ids;
        let chain_ids = ChainIds {
          chain_id: config.chain_id.unwrap_or(source.chain_id),
          espace_chain_id: config.espace_chain_id.unwrap_or(source.espace_chain_id),
          network_id: config.network_id.unwrap_or(source.network_id),
        };
        Self::from_fork(
          io,
          rpc,
          make_runtime_config(chain_ids),
          fork.cache,
          allocations,
        )
      }
    }
  }

  /// Executes Genesis on the calling thread and takes ownership of its Runtime.
  ///
  /// # Errors
  /// Returns the Genesis execution error without creating a node.
  fn from_genesis(
    io: Handle,
    config: RuntimeConfig,
    allocations: BTreeMap<AddressWithSpace, U256>,
    pos_definition: GenesisPosDefinition,
  ) -> Result<Self, NodeStartError> {
    let params = config.chain_spec.machine().params();
    let header = GenesisHeaderInput {
      author: GENESIS_ACCOUNT_ADDRESS,
      difficulty: U256::zero(),
      custom: params
        .custom_prefix(0)
        .expect("local execution rules must define Genesis custom data"),
      base_price: Some(params.init_base_price()),
    };
    let runtime = NodeRuntime::from_genesis(config, allocations, header, pos_definition)?;

    Ok(Self {
      io,
      state: NodeState::Running {
        runtime,
        read_task: None,
      },
    })
  }

  /// Extends a loaded base with the supplied local rules and startup balances.
  ///
  /// # Errors
  /// Returns an error if remote data cannot be loaded or the local execution
  /// settings cannot extend the selected base. No node is returned on failure.
  fn from_fork(
    io: Handle,
    rpc: ForkRpc,
    runtime_config: RuntimeConfig,
    cache: ForkCacheConfig,
    allocations: BTreeMap<AddressWithSpace, U256>,
  ) -> Result<Self, NodeStartError> {
    validate_execution_config(&runtime_config, rpc.base())?;
    let globals = io.block_on(rpc.global_parameters())?;
    let (client, mut read_task) = ForkReadTask::spawn(rpc, &io, cache);
    let runtime = match NodeRuntime::from_fork(runtime_config, client, globals, allocations) {
      Ok(runtime) => runtime,
      Err(source) => {
        let cleanup_error = io.block_on(read_task.close()).err();
        return Err(NodeStartError::ForkFunding {
          source,
          cleanup_error,
        });
      }
    };

    Ok(Self {
      io,
      state: NodeState::Running {
        runtime,
        read_task: Some(read_task),
      },
    })
  }

  pub(crate) fn runtime(&self) -> Result<&NodeRuntime, NodeError> {
    match &self.state {
      NodeState::Running { runtime, read_task }
        if read_task.as_ref().is_none_or(|task| !task.is_finished()) =>
      {
        Ok(runtime)
      }
      NodeState::Running { .. } => Err(NodeError::ForkServiceStopped(None)),
      NodeState::Stopping { .. } | NodeState::Closed(_) => Err(NodeError::Closed),
    }
  }

  pub(crate) fn runtime_mut(&mut self) -> Result<&mut NodeRuntime, NodeError> {
    match &mut self.state {
      NodeState::Running { runtime, read_task }
        if read_task.as_ref().is_none_or(|task| !task.is_finished()) =>
      {
        Ok(runtime)
      }
      NodeState::Running { .. } => Err(NodeError::ForkServiceStopped(None)),
      NodeState::Stopping { .. } | NodeState::Closed(_) => Err(NodeError::Closed),
    }
  }

  /// Waits for the read service and Runtime disposal, retaining the first join failure.
  /// Cancelling this wait preserves progress; a later call continues the close.
  pub(crate) async fn close(&mut self) -> Result<(), NodeError> {
    self.begin_close();
    let result = match &mut self.state {
      NodeState::Stopping {
        read_task,
        disposal,
        result,
      } => {
        if let Some(read_task) = read_task
          && let Err(source) = read_task.close().await
          && result.is_ok()
        {
          *result = Err(NodeError::ForkServiceStopped(Some(Arc::new(source))));
        }
        if let Err(source) = disposal.await
          && result.is_ok()
        {
          *result = Err(NodeError::RuntimeDisposalFailed(Arc::new(source)));
        }
        result.clone()
      }
      NodeState::Closed(result) => return result.clone(),
      NodeState::Running { .. } => unreachable!("close must first stop Runtime access"),
    };
    self.state = NodeState::Closed(result.clone());
    result
  }

  fn begin_close(&mut self) {
    self.state = match mem::replace(&mut self.state, NodeState::Closed(Ok(()))) {
      NodeState::Running { runtime, read_task } => {
        if let Some(read_task) = &read_task {
          read_task.request_stop();
        }
        let disposal = self.io.spawn_blocking(move || drop(runtime));
        NodeState::Stopping {
          read_task,
          disposal,
          result: Ok(()),
        }
      }
      state => state,
    };
  }
}

impl Drop for Node {
  fn drop(&mut self) {
    // Drop initiates cleanup; only an awaited close confirms completion.
    self.begin_close();
  }
}

fn validate_execution_config(
  config: &RuntimeConfig,
  base: &ForkBase,
) -> Result<(), NodeStartError> {
  let params = config.chain_spec.machine().params();
  let next_height = base.epoch_height + 1;
  // Local rules activate CIP-1559 at Genesis, so every fork extension needs parent fees.
  if base.base_price.is_none() {
    return Err(NodeStartError::UnsupportedConfiguration(
      "post-CIP-1559 execution requires parent base prices in both spaces",
    ));
  }

  if base.header_gas_limit < params.min_gas_limit {
    return Err(NodeStartError::UnsupportedConfiguration(
      "remote parent gas limit is below the local minimum",
    ));
  }

  config
    .production_defaults
    .build_environment(base.timestamp)
    .prepare_next_block(
      &config.production_defaults,
      base.timestamp,
      base.header_gas_limit,
      next_height,
      params,
    )?;

  Ok(())
}
