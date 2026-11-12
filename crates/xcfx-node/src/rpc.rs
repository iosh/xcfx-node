//! Inbound JSON-RPC protocol, independent of transport and Runtime ownership.

mod connection;
mod core;
mod dev;
mod dispatch;
mod error;
mod espace;
mod logs;
mod operation;
mod protocol;
mod service;
mod subscription;

pub(crate) use connection::{Connection, ConnectionHandle, RequestHandle};
pub(crate) use service::RpcService;

fn client_version() -> String {
  let info = crate::build_info();
  format!("xcfx-node/v{}/{}", info.version, info.target)
}
