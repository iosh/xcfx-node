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
  common::{shutdown_handler::shutdown, ClientTrait},
  full::FullClient,
  light::LightClient,
};
use parking_lot::{Condvar, Mutex};
use std::{env, fs, sync::Arc, thread};
use tempfile::{tempdir, TempDir};
mod config;
mod error;
use error::{NodeError, Result};

fn setup_node_configuration(
  config: config::ConfluxConfig,
  data_dir: &std::path::Path,
) -> Result<client::common::Configuration> {
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

struct NodeState {
  is_running: bool,
  shutdown_sender: Option<oneshot::Sender<()>>,
  thread_handle: Option<thread::JoinHandle<()>>,
}

#[napi]
pub struct ConfluxNode {
  state: Arc<TokioMutex<NodeState>>,
  temp_dir_handle: Arc<TokioMutex<Option<TempDir>>>,
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
}

#[napi]
impl ConfluxNode {
  #[napi(constructor)]
  pub fn new() -> Self {
    ConfluxNode {
      state: Arc::new(TokioMutex::new(NodeState {
        is_running: false,
        shutdown_sender: None,
        thread_handle: None,
      })),
      temp_dir_handle: Arc::new(TokioMutex::new(None)),
      exit_sign: Arc::new((Mutex::new(false), Condvar::new())),
    }
  }

  #[napi]
  pub async fn start_node(&self, config: config::ConfluxConfig) -> Result<()> {
    let mut state = self.state.lock().await;
    if state.is_running {
      return Err(NodeError::Runtime("Node is already running".to_string()));
    }

    let data_dir = match config.conflux_data_dir.as_ref() {
      Some(dir) => std::path::PathBuf::from(dir),
      None => {
        let temp_dir = tempdir().map_err(|e| {
          NodeError::Initialization(format!("Failed to create temp directory: {}", e))
        })?;
        let path = temp_dir.path().to_path_buf();
        *self.temp_dir_handle.lock().await = Some(temp_dir);
        path
      }
    };

    let conf = setup_node_configuration(config, &data_dir).map_err(|e| {
      NodeError::Configuration(format!("Failed to setup node configuration: {}", e))
    })?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let exit_sign = self.exit_sign.clone();
    let node_type = conf.node_type();

    let (startup_status_tx, startup_status_rx) =
      oneshot::channel::<std::result::Result<(), NodeError>>();

    let thread_handle = thread::spawn(move || {
      let client_result: Result<Box<dyn ClientTrait>> = match node_type {
        NodeType::Archive => ArchiveClient::start(conf, exit_sign.clone())
          .map(|client| client as Box<dyn ClientTrait>)
          .map_err(|e| NodeError::Runtime(format!("Failed to start Archive node: {}", e))),
        NodeType::Full => FullClient::start(conf, exit_sign.clone())
          .map(|client| client as Box<dyn ClientTrait>)
          .map_err(|e| NodeError::Runtime(format!("Failed to start Full node: {}", e))),
        NodeType::Light => LightClient::start(conf, exit_sign.clone())
          .map(|client| client as Box<dyn ClientTrait>)
          .map_err(|e| NodeError::Runtime(format!("Failed to start Light node: {}", e))),
        NodeType::Unknown => Err(NodeError::Configuration("Unknown node type".to_string())),
      };

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
        info!("Node started successfully");
        state.thread_handle = Some(thread_handle);
        state.shutdown_sender = Some(shutdown_tx);
        state.is_running = true;
        Ok(())
      }
      Ok(Err(e)) => {
        drop(state);
        let _ = shutdown_tx.send(());

        if let Err(join_err) = task::spawn_blocking(move || thread_handle.join()).await {
          warn!("Failed to join thread after startup failure: {}", join_err);
        }
        Err(e)
      }
      Err(_) => {
        let _ = shutdown_tx.send(());
        drop(state);

        match task::spawn_blocking(move || thread_handle.join()).await {
          Ok(Ok(_)) => Err(NodeError::Runtime(
            "Node thread exited prematurely without reporting startup status".to_string(),
          )),
          Ok(Err(panic_payload)) => Err(NodeError::Runtime(format!(
            "Node thread panicked: {:?}",
            panic_payload
          ))),
          Err(join_err) => Err(NodeError::Runtime(format!(
            "Failed to wait for node thread: {}",
            join_err
          ))),
        }
      }
    }
  }

  #[napi]
  pub async fn stop_node(&self) -> Result<()> {
    let mut state = self.state.lock().await;

    if !state.is_running {
      return Err(NodeError::Runtime("Node is not running".to_string()));
    }

    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();

    if let Some(sender) = state.shutdown_sender.take() {
      let _ = sender.send(());
    }

    let thread_handle = state.thread_handle.take();
    state.is_running = false;

    drop(state);

    if let Some(handle) = thread_handle {
      match task::spawn_blocking(move || handle.join()).await {
        Ok(Ok(_)) => {
          info!("Node thread completed successfully");
        }
        Ok(Err(panic_payload)) => {
          return Err(NodeError::Shutdown(format!(
            "Node thread panicked during shutdown: {:?}",
            panic_payload
          )));
        }
        Err(join_err) => {
          return Err(NodeError::Shutdown(format!(
            "Failed to wait for node thread: {}",
            join_err
          )));
        }
      }
    }
    *self.temp_dir_handle.lock().await = None;

    Ok(())
  }
}
