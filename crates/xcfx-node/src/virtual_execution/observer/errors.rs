use cfx_executor::{observer::CallTracer, stack::FrameResult};
use cfx_types::Address;
use cfx_vm_types::{ActionParams, CallType};

#[derive(Default)]
pub(super) struct ErrorObserver {
  calls: Vec<Address>,
  errors: Vec<(Address, String)>,
}

impl ErrorObserver {
  pub(super) fn into_errors(self) -> Vec<(Address, String)> {
    self.errors
  }

  fn record_result(&mut self, address: Address, result: &FrameResult) {
    let message = match result {
      Ok(result) if result.apply_state => {
        self.errors.clear();
        return;
      }
      Ok(result) => format!(
        "Vm reverted. {}",
        solidity_abi::string_revert_reason_decode(&result.return_data.to_vec()),
      ),
      Err(error) => format!("{error:?}"),
    };
    self.errors.push((address, message));
  }
}

impl CallTracer for ErrorObserver {
  fn record_call(&mut self, params: &ActionParams) {
    let address = match params.call_type {
      CallType::DelegateCall | CallType::CallCode => params.code_address,
      _ => params.address,
    };
    self.calls.push(address);
    self.errors.clear();
  }

  fn record_create(&mut self, _params: &ActionParams) {
    self.errors.clear();
  }

  fn record_call_result(&mut self, result: &FrameResult) {
    let address = self
      .calls
      .pop()
      .expect("call results must match observed calls");
    self.record_result(address, result);
  }

  fn record_create_result(&mut self, result: &FrameResult) {
    // The upstream unwind summary uses the zero address for a failed creation.
    let address = result
      .as_ref()
      .ok()
      .and_then(|result| result.create_address)
      .unwrap_or_default();
    self.record_result(address, result);
  }
}
