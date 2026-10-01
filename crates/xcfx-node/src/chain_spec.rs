use cfx_executor::{
  machine::{Machine, VmFactory},
  spec::{CommonParams, TransitionsEpochHeight},
};
use cfx_internal_common::ChainIdParamsInner;
use cfx_parameters::consensus_internal::{
  INITIAL_1559_CORE_BASE_PRICE, INITIAL_1559_ETH_BASE_PRICE, MINING_REWARD_TANZANITE_IN_UCFX,
};
use cfx_types::{AllChainID, SpaceMap, U256};
use diem_types::term_state::pos_state_config::PosStateConfig;
use std::{collections::BTreeMap, sync::Arc};

/// Chain and address-network identifiers for a source or local instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChainIds {
  pub(crate) chain_id: u32,
  pub(crate) espace_chain_id: u32,
  pub(crate) network_id: u64,
}

impl Default for ChainIds {
  fn default() -> Self {
    let chain_id = 201_029;
    Self {
      chain_id,
      espace_chain_id: 31_337,
      network_id: u64::from(chain_id),
    }
  }
}

/// Protocol rules shared by genesis and runtime execution.
pub(crate) struct ChainSpec {
  machine: Arc<Machine>,
  pos_state_config: PosStateConfig,
}

impl ChainSpec {
  /// Uses mainnet execution parameters with protocol upgrades active from Genesis.
  /// PoS view upgrades remain unscheduled; each instance owns its chain-ID lock.
  pub(crate) fn new(ids: ChainIds) -> Self {
    let params = CommonParams {
      network_id: ids.network_id,
      chain_id: ChainIdParamsInner::new_simple(AllChainID::new(ids.chain_id, ids.espace_chain_id)),
      base_block_rewards: BTreeMap::from([(0, U256::from(MINING_REWARD_TANZANITE_IN_UCFX))]),
      min_base_price: SpaceMap::new(
        U256::from(INITIAL_1559_CORE_BASE_PRICE),
        U256::from(INITIAL_1559_ETH_BASE_PRICE),
      ),
      transition_heights: TransitionsEpochHeight {
        // Upstream defaults all transitions to zero, including this test-only override.
        align_evm: u64::MAX,
        ..Default::default()
      },
      ..Default::default()
    };

    Self {
      machine: Arc::new(Machine::new_with_builtin(params, VmFactory::new(32 * 1024))),
      pos_state_config: PosStateConfig::default(),
    }
  }

  pub(crate) fn machine(&self) -> &Arc<Machine> {
    &self.machine
  }

  pub(crate) fn pos_state_config(&self) -> &PosStateConfig {
    &self.pos_state_config
  }
}
