//! Transport-independent JSON-RPC 2.0 envelopes and positional method decoding.
//!
//! JSON syntax is handled by Serde. This module defines protocol semantics only;
//! node lifecycle, execution, method dispatch and domain errors belong to callers.

mod params;
mod request;
mod response;

pub(super) use params::{deserialize_optional, deserialize_request, deserialize_single};
pub(super) use request::{Call, Id, Request};
pub(super) use response::{Response, RpcError, RpcResponse};
