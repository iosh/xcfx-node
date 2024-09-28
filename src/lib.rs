#![deny(clippy::all)]

use napi::Error;
use napi_derive::napi;

use cfxcore::NodeType;
use client::{
  archive::ArchiveClient,
  common::{shutdown_handler, ClientTrait},
  configuration::Configuration,
  full::FullClient,
  light::LightClient,
};
use log::info;
use parking_lot::{Condvar, Mutex};
use std::sync::{Arc, LazyLock};

static EXIT_SIGN: LazyLock<Arc<(Mutex<bool>, Condvar)>> =
  LazyLock::new(|| Arc::new((Mutex::new(false), Condvar::new())));

#[napi]
pub fn run_node() -> Result<(), napi::Error> {
  let mut conf = Configuration::default();
  conf.raw_conf.node_type = Some(NodeType::Full);
  conf.raw_conf.mode = Some("dev".to_string());

  let client_handle: Box<dyn ClientTrait>;
  client_handle = match conf.node_type() {
    NodeType::Archive => {
      info!("Starting archive client...");
      ArchiveClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start archive client: {}", e),
        )
      })?
    }
    NodeType::Full => {
      info!("Starting full client...");
      FullClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start full client: {}", e),
        )
      })?
    }
    NodeType::Light => {
      info!("Starting light client...");
      LightClient::start(conf, EXIT_SIGN.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start light client: {}", e),
        )
      })?
    }
    NodeType::Unknown => return Err(Error::new(napi::Status::InvalidArg, "Unknown node type")),
  };
  info!("Conflux client started");
  shutdown_handler::run(client_handle, EXIT_SIGN.clone());

  Ok(())
}

#[napi]
pub fn stop_node() {
  *EXIT_SIGN.0.lock() = true;
  EXIT_SIGN.1.notify_all();
}
