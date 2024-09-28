#![deny(clippy::all)]

use napi::Error;
use napi_derive::napi;

use cfxcore::NodeType;
use client::{
  archive::ArchiveClient,
  common::{shutdown_handler, ClientTrait},
  configuration::{Configuration, RawConfiguration},
  full::FullClient,
  light::LightClient,
};
use log::{info, LevelFilter};
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;

#[napi]
pub fn run_node() -> Result<(), napi::Error> {
  let mut conf = Configuration::default();
  let exit = Arc::new((Mutex::new(false), Condvar::new()));

  let client_handle: Box<dyn ClientTrait>;
  client_handle = match conf.node_type() {
    NodeType::Archive => {
      info!("Starting archive client...");
      ArchiveClient::start(conf, exit.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start archive client: {}", e),
        )
      })?
    }
    NodeType::Full => {
      info!("Starting full client...");
      FullClient::start(conf, exit.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start full client: {}", e),
        )
      })?
    }
    NodeType::Light => {
      info!("Starting light client...");
      LightClient::start(conf, exit.clone()).map_err(|e| {
        Error::new(
          napi::Status::Unknown,
          format!("failed to start light client: {}", e),
        )
      })?
    }
    NodeType::Unknown => return Err(Error::new(napi::Status::InvalidArg, "Unknown node type")),
  };
  info!("Conflux client started");
  shutdown_handler::run(client_handle, exit);

  Ok(())
}
