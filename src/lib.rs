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
use std::{env, fs, sync::Arc};
use tempfile::tempdir;
mod config;
mod error;
use error::{NodeError, Result};

use log4rs;

fn setup_env(
  config: config::ConfluxConfig,
  data_dir: &std::path::Path,
) -> Result<client::common::Configuration> {
  info!("Node working directory: {:?}", data_dir);

  // ensure directory exists
  fs::create_dir_all(data_dir).map_err(|e| {
    NodeError::InitializationError(format!("Failed to create data directory: {}", e))
  })?;

  env::set_current_dir(data_dir).map_err(|e| {
    NodeError::InitializationError(format!("Failed to set working directory: {}", e))
  })?;

  if let Some(ref log_conf) = config.log_conf {
    log4rs::init_file(log_conf, Default::default())
      .map_err(|e| NodeError::ConfigurationError(format!("Failed to initialize logging: {}", e)))?;
  };

  Ok(convert_config(config, data_dir))
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

    println!("start_node 111");
    let exit_sign = self.exit_sign.clone();

    println!("start_node 222");
    let thread_handle = thread::spawn(move || {
      // if data_dir is not set, use temp dir
      let data_dir = if let Some(dir) = config.data_dir.as_ref() {
        std::path::PathBuf::from(dir)
      } else {
        tempdir()
          .map_err(|e| {
            NodeError::InitializationError(format!("Failed to create temporary directory: {}", e))
          })?
          .into_path()
      };

      println!("start_node 333");

      let conf = setup_env(config, &data_dir)?;


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

      println!("start_node 444");

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
