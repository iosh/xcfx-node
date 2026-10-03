//! Startup, Runtime ownership, and shutdown for local and fork nodes.

mod command;
mod lifecycle;
mod mining;
mod service;
mod worker;

pub(crate) use command::{
  CallId, Cancellation, CommandError, CommandOutcome, MessageId, MessageOutcome, MineRequest,
  MineTiming, MiningError, MiningOutcome, NodeCommand, RequestError,
};
pub(crate) use lifecycle::NodeStatus;
pub(crate) use mining::{MiningCause, NodeEvent};
pub(crate) use service::{NodeEvents, NodeHandle, NodeRequest, NodeService};

use std::{
  collections::BTreeMap,
  fmt,
  panic::{AssertUnwindSafe, catch_unwind},
  sync::Arc,
};

use cfx_parameters::genesis::GENESIS_ACCOUNT_ADDRESS;
use cfx_types::{AddressSpaceUtil, AddressWithSpace, Space, U256};
use thiserror::Error;
use tokio::{runtime::Handle, task::JoinError};

use crate::{
  chain_spec::{ChainIds, ChainSpec},
  config::{ConfigError, MiningMode, NodeConfig},
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
  #[error("could not start the {thread} thread")]
  Thread {
    thread: &'static str,
    #[source]
    source: std::io::Error,
  },
  #[error("could not construct the node IO runtime")]
  IoRuntime(#[source] std::io::Error),
  #[error("node startup: {0}")]
  Service(#[from] NodeError),
  #[error("{source}; node service also failed: {service_error}")]
  WithServiceFailure {
    #[source]
    source: Box<NodeStartError>,
    service_error: NodeError,
  },
  #[error("configuration: {0}")]
  Configuration(#[from] ConfigError),
  #[error("configuration mining: {0}")]
  MiningInterval(#[from] command::InvalidMiningInterval),
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

impl NodeStartError {
  fn with_service_failure(self, service_error: NodeError) -> Self {
    match self {
      // Close retains the first service failure and can enrich its join details.
      // Use that final diagnosis instead of reporting the same failure twice.
      Self::Service(_) => Self::Service(service_error),
      source => Self::WithServiceFailure {
        source: Box::new(source),
        service_error,
      },
    }
  }
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
  #[error("node execution failed after an internal panic")]
  ExecutionPanicked,
  #[error("node IO supervision failed after an internal panic")]
  IoPanicked,
  #[error("node resource cleanup failed after an internal panic")]
  CleanupPanicked,
  #[error("node mining scheduler stopped unexpectedly")]
  SchedulerStopped(#[source] Option<Arc<JoinError>>),
}

impl fmt::Debug for NodeError {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(self, formatter)
  }
}

/// Owns one Runtime and, in fork mode, its remote read service.
///
/// Runtime calls require a thread that allows blocking. The instance's IO driver
/// stays active until `close` finishes reclaiming the node's resources.
struct Node {
  io: Handle,
  runtime: NodeRuntime,
  read_task: Option<ForkReadTask>,
}

impl Node {
  /// Resolves typed configuration and starts a local or fork Runtime.
  ///
  /// Runs synchronously on a thread that allows blocking, outside async tasks.
  /// The supplied IO runtime must remain active to serve remote reads and cleanup.
  ///
  /// # Errors
  /// Returns a stage-specific error without exposing a partially initialized node.
  /// If initial funding fails, the fork read service is closed before returning.
  ///
  /// # Panics
  /// Fork startup panics if called inside an async task.
  fn start(io: Handle, config: NodeConfig) -> Result<Self, NodeStartError> {
    config.validate()?;
    if let MiningMode::Interval(period) = config.mining {
      // Reject unusable timing before initialization. The scheduler computes a
      // fresh deadline afterwards so startup time does not consume the interval.
      mining::interval_deadline(period)?;
    }
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
      runtime,
      read_task: None,
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
      runtime,
      read_task: Some(read_task),
    })
  }

  fn runtime(&self) -> Result<&NodeRuntime, NodeError> {
    self.check_read_service()?;
    Ok(&self.runtime)
  }

  fn runtime_mut(&mut self) -> Result<&mut NodeRuntime, NodeError> {
    self.check_read_service()?;
    Ok(&mut self.runtime)
  }

  fn check_read_service(&self) -> Result<(), NodeError> {
    if self
      .read_task
      .as_ref()
      .is_some_and(ForkReadTask::is_finished)
    {
      Err(NodeError::ForkServiceStopped(None))
    } else {
      Ok(())
    }
  }

  /// Consumes the node on its execution thread. The IO driver must remain active
  /// until the read service has stopped. Service-level waiters do not own cleanup.
  fn close(self) -> Result<(), NodeError> {
    let Self {
      io,
      runtime,
      read_task,
    } = self;
    let read_result = catch_unwind(AssertUnwindSafe(|| match read_task {
      Some(mut task) => io
        .block_on(task.close())
        .map_err(|source| NodeError::ForkServiceStopped(Some(Arc::new(source)))),
      None => Ok(()),
    }))
    .unwrap_or(Err(NodeError::CleanupPanicked));
    // Runtime ownership stays outside the read-service unwind boundary, so its
    // disposal still runs if stopping or joining the service fails.
    let runtime_result =
      catch_unwind(AssertUnwindSafe(|| drop(runtime))).map_err(|_| NodeError::CleanupPanicked);
    read_result.and(runtime_result)
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
