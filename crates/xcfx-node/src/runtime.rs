//! Owns one node instance's authoritative protocol state.
use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::machine::Machine;
use cfx_types::{AddressWithSpace, U256};
use diem_types::term_state::pos_state_config::PosStateConfig;
use primitives::Block;

use crate::{
  genesis::{GenesisError, GenesisHeaderInput, execute_genesis_with_pos},
  pos::{CommittedPosState, GenesisPosDefinition},
  state::state_version::CommittedStateVersion,
};

pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  pos_config: PosStateConfig,
  genesis_block: Block,
  committed_state: CommittedStateVersion,
  committed_pos_state: CommittedPosState,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    machine: Arc<Machine>,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
    pos_definition: GenesisPosDefinition,
    pos_config: PosStateConfig,
  ) -> Result<Self, GenesisError> {
    let genesis = execute_genesis_with_pos(
      Arc::clone(&machine),
      allocations,
      header,
      &pos_definition,
      &pos_config,
    )?;

    Ok(Self {
      machine,
      pos_config,
      genesis_block: genesis.execution.block,
      committed_state: genesis.execution.committed_state,
      committed_pos_state: genesis.committed_pos_state,
    })
  }
}
