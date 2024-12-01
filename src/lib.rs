#![deny(clippy::all)]

use config::convert_config;
use log::{error, info};
use napi::{
  bindgen_prelude::*,
  threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode},
};
use napi_derive::napi;

use cfxcore::NodeType;
use client::{
  archive::ArchiveClient,
  common::{shutdown_handler, ClientTrait},
  full::FullClient,
  light::LightClient,
};
use parking_lot::{Condvar, Mutex};
use primitives::block_header::CIP112_TRANSITION_HEIGHT;
use std::thread;
use std::{env, sync::Arc};
use tempfile::{tempdir, TempDir};
mod config;

use log4rs;

fn setup_env(
  config: config::ConfluxConfig,
  temp_dir: &TempDir,
) -> Result<client::common::Configuration> {
  // we need set the work dir to temp dir
  let temp_dir_path = temp_dir.path();
  let conf = convert_config(config, &temp_dir_path);
  info!("node work dir: {:?}", temp_dir_path);
  env::set_current_dir(temp_dir_path).unwrap();

  if let Some(ref log_conf) = conf.raw_conf.log_conf {
    log4rs::init_file(log_conf, Default::default())
      .map_err(|e| {
        error!("failed to initialize log with log config file: {:?}", e);
        format!("failed to initialize log with log config file: {:?}", e)
      })
      .unwrap();
  };

  Ok(conf)
}

fn map_client_error(e: &str, js_callback: ThreadsafeFunction<Null>) -> Error {
  js_callback.call(
    Err(napi::Error::new(
      napi::Status::GenericFailure,
      format!("failed to start client: {}", e),
    )),
    ThreadsafeFunctionCallMode::NonBlocking,
  );
  Error::new(
    napi::Status::GenericFailure,
    format!("failed to start client: {}", e),
  )
}

#[napi]
pub struct ConfluxNode {
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
  thread_handle: Option<thread::JoinHandle<Result<(), Status>>>,
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
      let temp_dir = tempdir()?;
      let conf = setup_env(config, &temp_dir)?;

      let client_handle: Box<dyn ClientTrait>;
      client_handle = match conf.node_type() {
        NodeType::Archive => ArchiveClient::start(conf, exit_sign.clone())
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })
          .map_err(|e| map_client_error(&e, js_callback))?,
        NodeType::Full => FullClient::start(conf, exit_sign.clone())
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })
          .map_err(|e| map_client_error(&e, js_callback))?,
        NodeType::Light => LightClient::start(conf, exit_sign.clone())
          .inspect(|_| {
            js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
          })
          .map_err(|e| map_client_error(&e, js_callback))?,
        NodeType::Unknown => return Err(Error::new(napi::Status::InvalidArg, "Unknown node type")),
      };

      let mut lock = exit_sign.0.lock();
      if !*lock {
        exit_sign.1.wait(&mut lock);
      }

      shutdown_handler::run(client_handle, exit_sign.clone());
      Ok(())
    });
    self.thread_handle = Some(thread_handle);
  }

  #[napi]
  pub fn stop_node(&mut self, js_callback: ThreadsafeFunction<Null>) {
    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();

    let thread_handle = self.thread_handle.take().unwrap();

    match thread_handle.join().unwrap() {
      Ok(_) => {
        js_callback.call(Ok(Null), ThreadsafeFunctionCallMode::NonBlocking);
      }
      Err(e) => {
        js_callback.call(Err(e), ThreadsafeFunctionCallMode::NonBlocking);
      }
    }
  }
}
