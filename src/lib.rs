#![deny(clippy::all)]

use config::convert_config;
use napi::Error;
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
use tempfile::tempdir;
mod config;

#[napi]
pub struct ConfluxNode {
  exit_sign: Arc<(Mutex<bool>, Condvar)>,
}
#[napi]
impl ConfluxNode {
  #[napi(constructor)]
  pub fn new() -> Self {
    ConfluxNode {
      exit_sign: Arc::new((Mutex::new(false), Condvar::new())),
    }
  }
  #[napi]
  pub fn start_node(&self, config: config::ConfluxConfig) -> Result<(), napi::Error> {
    if CIP112_TRANSITION_HEIGHT.get().is_none() {
      CIP112_TRANSITION_HEIGHT.set(u64::MAX).expect("called once");
    }

    let exit_sign = self.exit_sign.clone();

    thread::spawn(move || {
      // we need set the work dir to temp dir
      let temp_dir = tempdir()?;
      let temp_dir_path = temp_dir.path();
      let conf = convert_config(config, &temp_dir_path);
      let temp_dir = tempdir().unwrap();
      env::set_current_dir(temp_dir.path()).unwrap();

      println!("current dir thread 1 {:?}", env::current_dir());
      let client_handle: Box<dyn ClientTrait>;
      client_handle = match conf.node_type() {
        NodeType::Archive => {
          println!("Starting archive client...");
          ArchiveClient::start(conf, exit_sign.clone()).map_err(|e| {
            eprintln!("failed to start archive client: {}", e);
            Error::new(
              napi::Status::Unknown,
              format!("failed to start archive client: {}", e),
            )
          })?
        }
        NodeType::Full => {
          println!("Starting full client...");

          FullClient::start(conf, exit_sign.clone()).map_err(|e| {
            eprintln!("failed to start full client: {}", e);
            Error::new(
              napi::Status::Unknown,
              format!("failed to start full client: {}", e),
            )
          })?
        }
        NodeType::Light => {
          println!("Starting light client...");
          LightClient::start(conf, exit_sign.clone()).map_err(|e| {
            Error::new(
              napi::Status::Unknown,
              format!("failed to start light client: {}", e),
            )
          })?
        }
        NodeType::Unknown => return Err(Error::new(napi::Status::InvalidArg, "Unknown node type")),
      };
      println!("Conflux client started");
      shutdown_handler::run(client_handle, exit_sign.clone());
      Ok(())
    });
    Ok(())
  }

  #[napi]
  pub fn stop_node(&self) {
    *self.exit_sign.0.lock() = true;
    self.exit_sign.1.notify_all();
  }
}
