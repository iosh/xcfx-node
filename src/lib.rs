#![deny(clippy::all)]
use log::{info, warn};
use napi::tokio::{
  sync::{oneshot, Mutex as TokioMutex},
  task,
};
use napi_derive::napi;

use cfxcore::NodeType;
use client::{
  archive::ArchiveClient,
  common::{shutdown_handler::shutdown, ClientTrait, Configuration},
  full::FullClient,
  light::LightClient,
};
use parking_lot::{Condvar, Mutex};
use std::{env, fs, path::Path, sync::Arc};
use tempfile::{tempdir, TempDir};
mod config;
mod error;
use error::{NodeError, Result};

struct NodeLifecycle {
  thread_handle: task::JoinHandle<()>,
  shutdown_sender: oneshot::Sender<()>,
  _temp_dir: Option<TempDir>,
}

impl NodeLifecycle {
  fn new(
    thread_handle: task::JoinHandle<()>,
    shutdown_sender: oneshot::Sender<()>,
    _temp_dir: Option<TempDir>,
  ) -> Self {
    NodeLifecycle {
      thread_handle,
      shutdown_sender,
      _temp_dir,
    }
  }

  async fn shutdown(self) -> Result<()> {
    let _ = self.shutdown_sender.send(());

    match self.thread_handle.await {
      Ok(_) => {
        info!("Node thread completed successfully");
        Ok(())
      }
      Err(e) if e.is_cancelled() => Ok(()),
      Err(e) if e.is_panic() => {
        warn!("Node thread panicked: {:?}", e);
        Err(NodeError::Shutdown("Node thread panicked".to_string()))
      }
      Err(e) => Err(NodeError::Shutdown(format!(
        "Failed to wait for node thread: {}",
        e
      ))),
    }
  }
}

#[napi]
pub struct ConfluxNode {
  lifecycle: Arc<TokioMutex<Option<NodeLifecycle>>>,
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
}

#[napi]
impl ConfluxNode {
  #[napi(constructor)]
  pub fn new() -> Self {
    ConfluxNode {
      lifecycle: Arc::new(TokioMutex::new(None)),
      exit_sign: Arc::new((Mutex::new(false), Condvar::new())),
    }
  }

  #[napi]
  pub async fn start_node(&self, config: config::ConfluxConfig) -> Result<()> {
    let mut lifecycle_guard = self.lifecycle.lock().await;

    if lifecycle_guard.is_some() {
      return Err(NodeError::Runtime("Node is already running".to_string()));
    }

    let (data_dir, temp_dir) = self.prepare_data_directory(&config)?;

    let conf = self.setup_configuration(&config, &data_dir)?;

    let lifecycle = self.spawn_node_async(conf, temp_dir).await?;
    *lifecycle_guard = Some(lifecycle);

    info!("Node started successfully");

    Ok(())
  }

  #[napi]
  pub async fn stop_node(&self) -> Result<()> {
    let lifecycle = {
      let mut lifecycle_guard = self.lifecycle.lock().await;
      match lifecycle_guard.take() {
        Some(lifecycle) => lifecycle,
        None => return Err(NodeError::Runtime("Node is not running".to_string())),
      }
    };

    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();

    lifecycle.shutdown().await?;
    info!("Node shutdown complete");

    Ok(())
  }

  async fn spawn_node_async(
    &self,
    conf: Configuration,
    temp_dir: Option<TempDir>,
  ) -> Result<NodeLifecycle> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let (startup_status_tx, startup_status_rx) =
      oneshot::channel::<std::result::Result<(), NodeError>>();

    let exit_sign = self.exit_sign.clone();

    let join_handle = task::spawn_blocking(move || {
      let client_result = ConfluxNode::create_client(conf, exit_sign);

      match client_result {
        Ok(client) => {
          if startup_status_tx.send(Ok(())).is_err() {
            warn!("Failed to send successful startup signal to start_node; main task might have been cancelled. Shutting down node.");
            shutdown(client);
            return;
          }

          // Wait for shutdown signal
          let _ = shutdown_rx.blocking_recv();

          info!("Node is shutting down");
          shutdown(client);
          info!("Node shutdown complete");
        }
        Err(e) => {
          if startup_status_tx.send(Err(e)).is_err() {
            warn!("Failed to send startup error signal to start_node; error may not be propagated to caller.");
          }
        }
      }
    });

    match startup_status_rx.await {
      Ok(Ok(())) => {
        let thread_handle = join_handle;
        Ok(NodeLifecycle::new(thread_handle, shutdown_tx, temp_dir))
      }
      Ok(Err(e)) => {
        join_handle.abort();
        Err(e)
      }
      Err(_) => {
        join_handle.abort();
        Err(NodeError::Runtime(
          "Node startup communication failed".to_string(),
        ))
      }
    }
  }

  fn create_client(
    conf: Configuration,
    exit_sign: Arc<(Mutex<bool>, Condvar)>,
  ) -> Result<Box<dyn ClientTrait>> {
    match conf.node_type() {
      NodeType::Archive => ArchiveClient::start(conf, exit_sign)
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Archive node: {}", e))),
      NodeType::Full => FullClient::start(conf, exit_sign)
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Full node: {}", e))),
      NodeType::Light => LightClient::start(conf, exit_sign)
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Light node: {}", e))),
      NodeType::Unknown => Err(NodeError::Configuration("Unknown node type".to_string())),
    }
  }

  fn setup_configuration(
    &self,
    config: &config::ConfluxConfig,
    data_dir: &Path,
  ) -> Result<Configuration> {
    info!("Node working directory: {:?}", data_dir);

    // ensure directory exists
    fs::create_dir_all(data_dir)
      .map_err(|e| NodeError::Initialization(format!("Failed to create data directory: {}", e)))?;

    // set working directory， some file use relative path
    env::set_current_dir(data_dir)
      .map_err(|e| NodeError::Initialization(format!("Failed to set working directory: {}", e)))?;

    if let Some(ref log_conf) = config.log_conf {
      log4rs::init_file(log_conf, Default::default())
        .map_err(|e| NodeError::Configuration(format!("Failed to initialize logging: {}", e)))?;
    };

    config
      .to_configuration(data_dir)
      .map_err(|e| NodeError::Configuration(format!("Failed to convert config: {}", e)))
  }

  fn prepare_data_directory(
    &self,
    config: &config::ConfluxConfig,
  ) -> Result<(std::path::PathBuf, Option<TempDir>)> {
    match config.conflux_data_dir.as_ref() {
      Some(dir) => Ok((std::path::PathBuf::from(dir), None)),
      None => {
        let temp_dir = tempdir().map_err(|e| {
          NodeError::Initialization(format!("Failed to create temp directory: {}", e))
        })?;
        let path = temp_dir.path().to_path_buf();
        Ok((path, Some(temp_dir)))
      }
    }
  }
}
