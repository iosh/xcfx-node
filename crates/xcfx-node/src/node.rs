//! Startup, Runtime ownership, and shutdown for local and fork nodes.

use std::{collections::BTreeMap, fmt, mem, sync::Arc};

use cfx_parameters::consensus_internal::ELASTICITY_MULTIPLIER;
use cfx_types::{AddressWithSpace, U256};
use thiserror::Error;
use tokio::{
  runtime::Handle,
  task::{JoinError, JoinHandle},
};

use crate::{
  fork::{ForkBase, ForkConfig, ForkLoadError, ForkReadError, ForkReadTask, ForkRpc},
  genesis::{GenesisError, GenesisHeaderInput},
  pos::GenesisPosDefinition,
  production_environment::ProductionTimeError,
  rpc_client::ConfluxRpcClient,
  runtime::{NodeRuntime, RuntimeConfig},
};

#[derive(Debug, Error)]
pub(crate) enum ForkStartError {
  #[error(transparent)]
  Load(#[from] ForkLoadError),
  #[error(transparent)]
  InitialState(#[from] ForkReadError),
  #[error(transparent)]
  Production(#[from] ProductionTimeError),
  #[error("unsupported fork execution configuration: {0}")]
  UnsupportedConfiguration(&'static str),
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
  /// Executes Genesis on the calling thread and takes ownership of its Runtime.
  ///
  /// # Errors
  /// Returns the Genesis execution error without creating a node.
  pub(crate) fn from_genesis(
    io: Handle,
    config: RuntimeConfig,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
  ) -> Result<Self, GenesisError> {
    let runtime = NodeRuntime::from_genesis(config, allocations, header, pos_definition)?;

    Ok(Self {
      io,
      state: NodeState::Running {
        runtime,
        read_task: None,
      },
    })
  }

  /// Loads the base and required initial state before starting the read service.
  ///
  /// # Errors
  /// Returns an error if remote data cannot be loaded or the local execution
  /// settings cannot extend the selected base. No node is returned on failure.
  pub(crate) async fn from_fork(
    io: Handle,
    rpc: ConfluxRpcClient,
    runtime_config: RuntimeConfig,
    config: ForkConfig,
  ) -> Result<Self, ForkStartError> {
    let rpc = ForkRpc::connect(rpc, config.epoch).await?;
    validate_execution_config(&runtime_config, rpc.base())?;
    let globals = rpc.global_parameters().await?;
    let (client, read_task) = ForkReadTask::spawn(rpc, &io, config.cache);
    let runtime = NodeRuntime::from_fork(runtime_config, client, globals);

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
) -> Result<(), ForkStartError> {
  let params = config.chain_spec.machine().params();
  let next_height = base.epoch_height + 1;
  let transition = params.transition_heights.cip1559;
  if next_height < transition {
    return Err(ForkStartError::UnsupportedConfiguration(
      "local block production requires CIP-1559 at the first local epoch",
    ));
  }
  if next_height > transition && base.base_price.is_none() {
    return Err(ForkStartError::UnsupportedConfiguration(
      "post-CIP-1559 execution requires parent base prices in both spaces",
    ));
  }

  // At activation the producer doubles the parent limit. The loader bounds it
  // through the eSpace header's u64 gas projection, so this fits in U256.
  let parent_gas_limit = if next_height == transition {
    base.header_gas_limit * U256::from(ELASTICITY_MULTIPLIER as u64)
  } else {
    base.header_gas_limit
  };
  if parent_gas_limit < params.min_gas_limit
    || (parent_gas_limit / params.gas_limit_bound_divisor).is_zero()
  {
    return Err(ForkStartError::UnsupportedConfiguration(
      "remote parent gas limit is incompatible with the local gas bounds",
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
