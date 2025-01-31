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
use primitives::block_header::CIP112_TRANSITION_HEIGHT;
use std::thread;
use std::{env, sync::Arc};
use tempfile::{tempdir, TempDir};
mod config;
mod error;
use error::{NodeError, Result};

use log4rs;

fn setup_env(
  config: config::ConfluxConfig,
  temp_dir: &TempDir,
) -> Result<client::common::Configuration> {
  let temp_dir_path = temp_dir.path();
  let conf = convert_config(config, &temp_dir_path);
  info!("Node working directory: {:?}", temp_dir_path);

  env::set_current_dir(temp_dir_path).map_err(|e| {
    NodeError::InitializationError(format!("Failed to set working directory: {}", e))
  })?;

  if let Some(ref log_conf) = conf.raw_conf.log_conf {
    log4rs::init_file(log_conf, Default::default())
      .map_err(|e| NodeError::ConfigurationError(format!("Failed to initialize logging: {}", e)))?;
  };

  Ok(conf)
}

#[napi]
pub struct ConfluxNode {
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
  thread_handle: Option<thread::JoinHandle<std::result::Result<(), Error>>>,
}
#[napi]
impl ConfluxNode {
  #[napi(constructor)]
  pub fn new() -> Self {
    ConfluxNode {
      exit_sign: Arc::new((Mutex::new(false), Condvar::new())),
      thread_handle: None,
    }
  }
  #[napi]
  pub fn start_node(
    &mut self,
    config: config::ConfluxConfig,
    js_callback: ThreadsafeFunction<Null>,
  ) {
    if CIP112_TRANSITION_HEIGHT.get().is_none() {
      CIP112_TRANSITION_HEIGHT.set(u64::MAX).expect("called once");
    }

    let exit_sign = self.exit_sign.clone();

    let thread_handle = thread::spawn(move || {
      let temp_dir = tempdir().map_err(|e| {
        NodeError::InitializationError(format!("Failed to create temporary directory: {}", e))
      })?;

      let conf = setup_env(config, &temp_dir)?;

      let client_handle: Box<dyn ClientTrait> = match conf.node_type() {
        NodeType::Archive => ArchiveClient::start(conf, exit_sign.clone())
          .map_err(|e| NodeError::RuntimeError(format!("Failed to start Archive node: {}", e)))
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })?,
        NodeType::Full => FullClient::start(conf, exit_sign.clone())
          .map_err(|e| NodeError::RuntimeError(format!("Failed to start Full node: {}", e)))
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })?,
        NodeType::Light => LightClient::start(conf, exit_sign.clone())
          .map_err(|e| NodeError::RuntimeError(format!("Failed to start Light node: {}", e)))
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })?,
        NodeType::Unknown => {
          return Err(NodeError::ConfigurationError("Unknown node type".to_string()).into());
        }
      };

      let mut lock = exit_sign.0.lock();
      if !*lock {
        exit_sign.1.wait(&mut lock);
      }

      shutdown(client_handle);
      Ok(())
    });

    self.thread_handle = Some(thread_handle);
  }

  #[napi]
  pub fn stop_node(&mut self, js_callback: ThreadsafeFunction<Null>) {
    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();

    if let Some(thread_handle) = self.thread_handle.take() {
      match thread_handle.join() {
        Ok(result) => match result {
          Ok(_) => {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          }
          Err(e) => {
            js_callback.call(
              Err(NodeError::ShutdownError(format!("Failed to shutdown node: {:?}", e)).into()),
              ThreadsafeFunctionCallMode::NonBlocking,
            );
          }
        },
        Err(_e) => {
          js_callback.call(
            Err(NodeError::ShutdownError("Thread terminated abnormally".to_string()).into()),
            ThreadsafeFunctionCallMode::NonBlocking,
          );
        }
      }
    }
  }
}
