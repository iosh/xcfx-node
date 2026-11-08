//! Conflux execution state with caller-owned persistence and complete recovery.
#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate cfx_util_macros;

pub mod account;
pub mod database;
mod execution_state;
mod state;

use cfx_executor::{executive_observer, internal_contract, substate};
pub use cfx_executor::{return_if, try_loaded, unwrap_or_return};
use cfx_types::{AddressWithSpace, U256};
pub use state::{
  initialize_cip107, initialize_cip137, initialize_or_update_dao_voted_params, State,
};

/// A changed account's nonce, emitted by the same writeback as its state version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountNonce {
  pub address: AddressWithSpace,
  pub nonce: U256,
}
