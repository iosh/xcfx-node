mod access_list;
mod errors;
mod gas;

pub(super) use access_list::AccessListObserver;

use cfx_executor::{
  executive::Executed,
  observer::{
    AsTracer, CallTracer, CheckpointTracer, DrainTrace, InternalTransferTracer, OpcodeTracer,
    SetAuthTracer, StorageTracer, TracerTrait,
  },
  stack::FrameResult,
  state::State,
};
use cfx_parity_trace_types::SetAuth;
use cfx_types::{Address, Space, U256};
use cfx_vm_types::{ActionParams, InterpreterInfo};
use primitives::AccessList;
use typemap::ShareDebugMap;

use errors::ErrorObserver;
use gas::GasObserver;

#[derive(Debug)]
struct CollectedObservation {
  gas_required: U256,
  access_list: Option<AccessListObserver>,
  errors: Option<Vec<(Address, String)>>,
}

struct ObservationKey;

impl typemap::Key for ObservationKey {
  type Value = CollectedObservation;
}

/// Completed observations paired with the execution that produced them.
pub(super) struct ObservedExecution {
  pub(super) executed: Executed,
  pub(super) gas_required: U256,
  pub(super) access_list: Option<AccessList>,
  pub(super) errors: Option<Vec<(Address, String)>>,
}

impl ObservedExecution {
  /// Resolves state-dependent observations after restoring the prepared state.
  pub(super) fn new(
    mut executed: Executed,
    prepared_state: &State<'_>,
  ) -> cfx_statedb::Result<Self> {
    let CollectedObservation {
      gas_required,
      access_list,
      errors,
    } = executed
      .ext_result
      .remove::<ObservationKey>()
      .expect("every simulated Executed is produced by ExecutionObserver");
    let access_list = access_list
      .map(|observer| observer.into_access_list(prepared_state))
      .transpose()?;
    Ok(Self {
      executed,
      gas_required,
      access_list,
      errors,
    })
  }
}

pub(super) struct ExecutionObserver {
  gas: GasObserver,
  access_list: Option<AccessListObserver>,
  errors: Option<ErrorObserver>,
}

impl ExecutionObserver {
  pub(super) fn new(space: Space, access_list: Option<AccessListObserver>) -> Self {
    Self {
      gas: GasObserver::default(),
      access_list,
      errors: (space == Space::Native).then(ErrorObserver::default),
    }
  }
}

impl AsTracer for ExecutionObserver {
  fn as_tracer<'a>(&'a mut self) -> Box<dyn 'a + TracerTrait> {
    Box::new(self)
  }
}

impl DrainTrace for ExecutionObserver {
  fn drain_trace(self, map: &mut ShareDebugMap) {
    map.insert::<ObservationKey>(CollectedObservation {
      gas_required: self.gas.required(),
      access_list: self.access_list,
      errors: self.errors.map(ErrorObserver::into_errors),
    });
  }
}

impl CallTracer for ExecutionObserver {
  fn record_call(&mut self, params: &ActionParams) {
    self.gas.record_call(params);
    if let Some(errors) = &mut self.errors {
      errors.record_call(params);
    }
  }

  fn record_call_result(&mut self, result: &FrameResult) {
    self.gas.record_call_result(result);
    if let Some(errors) = &mut self.errors {
      errors.record_call_result(result);
    }
  }

  fn record_create(&mut self, params: &ActionParams) {
    self.gas.record_create(params);
    if let Some(errors) = &mut self.errors {
      errors.record_create(params);
    }
  }

  fn record_create_result(&mut self, result: &FrameResult) {
    self.gas.record_create_result(result);
    if let Some(errors) = &mut self.errors {
      errors.record_create_result(result);
    }
  }
}

impl OpcodeTracer for ExecutionObserver {
  fn do_trace_opcode(&self, enabled: &mut bool) {
    if self.access_list.is_some() {
      *enabled = true;
    }
  }

  fn step(&mut self, interp: &dyn InterpreterInfo) {
    if let Some(access_list) = &mut self.access_list {
      access_list.step(interp);
    }
  }
}

impl CheckpointTracer for ExecutionObserver {}
impl InternalTransferTracer for ExecutionObserver {}
impl StorageTracer for ExecutionObserver {}
impl SetAuthTracer for ExecutionObserver {
  fn record_set_auth(&mut self, action: SetAuth) {
    if let Some(access_list) = &mut self.access_list {
      access_list.record_set_auth(action);
    }
  }
}
