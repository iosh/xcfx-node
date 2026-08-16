//! Owns one node instance's authoritative protocol state.

use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::machine::Machine;
use cfx_types::{AddressWithSpace, U256};
use primitives::Block;

use crate::{
  genesis::{GenesisError, GenesisHeaderInput, execute_genesis},
  state::state_version::CommittedStateVersion,
};

pub(crate) struct NodeRuntime {
  machine: Arc<Machine>,
  genesis_block: Block,
  committed_state: CommittedStateVersion,
}

impl NodeRuntime {
  pub(crate) fn from_genesis(
    machine: Arc<Machine>,
    allocations: BTreeMap<AddressWithSpace, U256>,
    header: GenesisHeaderInput,
  ) -> Result<Self, GenesisError> {
    let genesis = execute_genesis(Arc::clone(&machine), allocations, header)?;

    Ok(Self {
      machine,
      genesis_block: genesis.block,
      committed_state: genesis.committed_state,
    })
  }
}

#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc};

  use cfx_executor::{
    machine::{Machine, VmFactory},
    spec::CommonParams,
  };
  use cfx_internal_common::ChainIdParamsInner;
  use cfx_parameters::genesis::GENESIS_ACCOUNT_ADDRESS;
  use cfx_types::{AllChainID, U256};

  use super::NodeRuntime;
  use crate::genesis::GenesisHeaderInput;

  const TEST_CORE_CHAIN_ID: u32 = 71;
  const TEST_ESPACE_CHAIN_ID: u32 = 72;

  fn test_machine() -> Arc<Machine> {
    let mut params = CommonParams::default();
    params.chain_id =
      ChainIdParamsInner::new_simple(AllChainID::new(TEST_CORE_CHAIN_ID, TEST_ESPACE_CHAIN_ID));

    Arc::new(Machine::new_with_builtin(params, VmFactory::new(32 * 1024)))
  }

  #[test]
  fn initializes_consistent_genesis_state() {
    let runtime = NodeRuntime::from_genesis(
      test_machine(),
      BTreeMap::new(),
      GenesisHeaderInput {
        author: GENESIS_ACCOUNT_ADDRESS,
        difficulty: U256::zero(),
      },
    )
    .unwrap();

    for transaction in &runtime.genesis_block.transactions {
      assert_eq!(transaction.chain_id(), Some(TEST_CORE_CHAIN_ID));
    }

    let state_root = runtime.committed_state.version.root_with_aux_info();

    assert_eq!(
      runtime.genesis_block.hash(),
      runtime.committed_state.epoch_id
    );
    assert_eq!(
      *runtime.genesis_block.block_header.deferred_state_root(),
      state_root.aux_info.state_root_hash
    );
  }
}
