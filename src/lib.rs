#![deny(clippy::all)]

use config::convert_config;
use log::info;
use napi::{
  bindgen_prelude::*,
  threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
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
use std::{env, fs, sync::Arc};
use tempfile::{tempdir, TempDir};
mod config;
mod error;
use error::{NodeError, Result};

fn setup_env(
  config: config::ConfluxConfig,
  data_dir: &std::path::Path,
) -> Result<client::common::Configuration> {
  info!("Node working directory: {:?}", data_dir);

  // ensure directory exists
  fs::create_dir_all(data_dir)
    .map_err(|e| NodeError::Initialization(format!("Failed to create data directory: {}", e)))?;

  env::set_current_dir(data_dir)
    .map_err(|e| NodeError::Initialization(format!("Failed to set working directory: {}", e)))?;

  if let Some(ref log_conf) = config.log_conf {
    log4rs::init_file(log_conf, Default::default())
      .map_err(|e| NodeError::Configuration(format!("Failed to initialize logging: {}", e)))?;
  };

  Ok(convert_config(config, data_dir))
}

#[napi]
pub struct ConfluxNode {
  client_handle: Option<Box<dyn ClientTrait>>,
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
  temp_dir_handle: Option<TempDir>,
}

#[napi]
impl ConfluxNode {
  #[napi(constructor)]
  pub fn new() -> Self {
    ConfluxNode {
      client_handle: None,
      exit_sign: Arc::new((Mutex::new(false), Condvar::new())),
      temp_dir_handle: None,
    }
  }

  fn handle_error(error: NodeError, js_callback: &ThreadsafeFunction<Null>) {
    js_callback.call(Err(error.into()), ThreadsafeFunctionCallMode::NonBlocking);
  }

  #[napi]
  pub fn start_node(
    &mut self,
    config: config::ConfluxConfig,
    js_callback: ThreadsafeFunction<Null>,
  ) {
    // if data_dir is not set, use temp dir
    let data_dir = match config.data_dir.as_ref().map_or_else(
      || {
        tempdir().map(|dir| {
          self.temp_dir_handle = Some(dir);
          self.temp_dir_handle.as_ref().unwrap().path().to_path_buf()
        })
      },
      |dir| Ok(std::path::PathBuf::from(dir)),
    ) {
      Ok(dir) => dir,
      Err(e) => {
        Self::handle_error(
          NodeError::Initialization(format!("Failed to create directory: {}", e)),
          &js_callback,
        );
        return;
      }
    };

    let conf = match setup_env(config, &data_dir) {
      Ok(conf) => conf,
      Err(e) => {
        Self::handle_error(e, &js_callback);
        return;
      }
    };

    let client_handle: Result<Box<dyn ClientTrait>> = match conf.node_type() {
      NodeType::Archive => ArchiveClient::start(conf, self.exit_sign.clone())
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Archive node: {}", e))),
      NodeType::Full => FullClient::start(conf, self.exit_sign.clone())
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Full node: {}", e))),
      NodeType::Light => LightClient::start(conf, self.exit_sign.clone())
        .map(|client| client as Box<dyn ClientTrait>)
        .map_err(|e| NodeError::Runtime(format!("Failed to start Light node: {}", e))),
      NodeType::Unknown => Err(NodeError::Configuration("Unknown node type".to_string())),
    };

    match client_handle {
      Ok(handle) => {
        self.client_handle = Some(handle);
        js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
      }
      Err(e) => {
        Self::handle_error(e, &js_callback);
      }
    }
  }

  #[napi]
  pub fn stop_node(&mut self, js_callback: ThreadsafeFunction<Null>) {
    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();

    if let Some(client_handle) = self.client_handle.take() {
      shutdown(client_handle);
      self.temp_dir_handle = None;
      js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
    } else {
      js_callback.call(
        Err(NodeError::Runtime("Node is not running".to_string()).into()),
        ThreadsafeFunctionCallMode::NonBlocking,
      );
    }
  }
}
