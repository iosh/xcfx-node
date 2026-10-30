use cfx_executor::{observer::CallTracer, stack::FrameResult};
use cfx_parameters::{
  block::CROSS_SPACE_GAS_RATIO, internal_contract_addresses::CROSS_SPACE_CONTRACT_ADDRESS,
};
use cfx_types::{Space, U256};
use cfx_vm_types::ActionParams;

struct GasFrame {
  initial: U256,
  children_used: U256,
  children_required: U256,
  cross_space: bool,
}

#[derive(Default)]
pub(super) struct GasObserver {
  frames: Vec<GasFrame>,
  required: U256,
}

impl GasObserver {
  pub(super) fn required(&self) -> U256 {
    self.required
  }

  fn enter(&mut self, initial: U256, cross_space: bool) {
    self.frames.push(GasFrame {
      initial,
      children_used: U256::zero(),
      children_required: U256::zero(),
      cross_space,
    });
  }

  fn finish(&mut self, result: &FrameResult) {
    let frame = self
      .frames
      .pop()
      .expect("every execution frame has a matching entry");
    let remaining = result
      .as_ref()
      .map_or(U256::zero(), |result| result.gas_left);
    // A value-transfer stipend may leave more gas than the frame received.
    let used = frame.initial.saturating_sub(remaining);
    let own_used = used.saturating_sub(frame.children_used);
    let (numerator, denominator) = if frame.cross_space {
      (CROSS_SPACE_GAS_RATIO, 1)
    } else {
      (64, 63)
    };
    // Reserve enough parent gas to survive the protocol's forwarding ratio.
    let required = own_used + (frame.children_required * numerator + denominator - 1) / denominator;
    if let Some(parent) = self.frames.last_mut() {
      parent.children_used += used;
      parent.children_required += required;
    } else {
      self.required = required;
    }
  }
}

impl CallTracer for GasObserver {
  fn record_call(&mut self, params: &ActionParams) {
    self.enter(
      params.gas,
      params.space == Space::Native && params.code_address == CROSS_SPACE_CONTRACT_ADDRESS,
    );
  }

  fn record_create(&mut self, params: &ActionParams) {
    self.enter(params.gas, false);
  }

  fn record_call_result(&mut self, result: &FrameResult) {
    self.finish(result);
  }

  fn record_create_result(&mut self, result: &FrameResult) {
    self.finish(result);
  }
}
