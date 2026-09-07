mod pos_parameters;

pub(crate) use pos_parameters::PosParameters;

use cfx_executor::{
  machine::{Machine, VmFactory},
  spec::CommonParams,
};
use cfx_internal_common::ChainIdParamsInner;
use diem_types::term_state::pos_state_config::PosStateConfig;
use std::sync::Arc;

/// Protocol rules shared by genesis and runtime execution.
pub(crate) struct ChainSpec {
  machine: Arc<Machine>,
  pos_state_config: PosStateConfig,
}

impl ChainSpec {
  pub(crate) fn new(mut params: CommonParams, pos: PosParameters, vm_factory: VmFactory) -> Self {
    // CommonParams::clone shares the chain-ID lock; keep this instance's schedule independent.
    let chain_id = params.chain_id.read().clone();
    params.chain_id = ChainIdParamsInner::new_from_inner(&chain_id);

    Self {
      machine: Arc::new(Machine::new_with_builtin(params, vm_factory)),
      pos_state_config: pos.into_config(),
    }
  }

  pub(crate) fn machine(&self) -> &Arc<Machine> {
    &self.machine
  }

  pub(crate) fn pos_state_config(&self) -> &PosStateConfig {
    &self.pos_state_config
  }
}
