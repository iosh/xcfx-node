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
use log::info;
use parking_lot::{Condvar, Mutex};
use primitives::block_header::CIP112_TRANSITION_HEIGHT;
use std::thread;
use std::{
  env,
  sync::{Arc, LazyLock},
};
use tempfile::tempdir;
mod config;
static EXIT_SIGN: LazyLock<Arc<(Mutex<bool>, Condvar)>> =
  LazyLock::new(|| Arc::new((Mutex::new(false), Condvar::new())));

#[napi]
pub fn run_node(config: config::ConfluxConfig) -> Result<(), napi::Error> {
  let temp_dir = tempdir()?;
  let temp_dir_path = temp_dir.path();
  let conf = convert_config(config, &temp_dir_path);
  CIP112_TRANSITION_HEIGHT.set(u64::MAX).expect("called once");

  thread::spawn(|| {
    // we need set the work dir to temp dir

    let temp_dir = tempdir().unwrap();
    env::set_current_dir(temp_dir.path()).unwrap();

    println!("current dir thread 1 {:?}", env::current_dir());
    let client_handle: Box<dyn ClientTrait>;
    client_handle = match conf.node_type() {
      NodeType::Archive => {
        println!("Starting archive client...");
        ArchiveClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
          Error::new(
            napi::Status::Unknown,
            format!("failed to start archive client: {}", e),
          )
        })?
      }
      NodeType::Full => {
        println!("Starting full client...");
        FullClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
          Error::new(
            napi::Status::Unknown,
            format!("failed to start full client: {}", e),
          )
        })?
      }
      NodeType::Light => {
        println!("Starting light client...");
        LightClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
          Error::new(
            napi::Status::Unknown,
            format!("failed to start light client: {}", e),
          )
        })?
      }
      NodeType::Unknown => return Err(Error::new(napi::Status::InvalidArg, "Unknown node type")),
    };
    println!("Conflux client started");
    shutdown_handler::run(client_handle, EXIT_SIGN.clone());
    Ok(())
  });
  Ok(())
}

#[napi]
pub fn stop_node() {
  *EXIT_SIGN.0.lock() = true;
  EXIT_SIGN.1.notify_all();
}
