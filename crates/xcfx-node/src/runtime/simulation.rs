use thiserror::Error;

use crate::{
  transaction_request::TransactionRequest,
  virtual_execution::{
    AccessListResult, Simulation, SimulationRequest, TransactionEstimate, VirtualExecutionError,
    VirtualExecutionOverrides,
  },
};

use super::{NodeRuntime, StateSelectionError, StateSelector};

#[derive(Debug, Error)]
pub(crate) enum RuntimeSimulationError {
  #[error(transparent)]
  Selection(#[from] StateSelectionError),
  #[error(transparent)]
  Simulation(#[from] VirtualExecutionError),
}

impl NodeRuntime {
  fn espace_simulation(
    &self,
    request: TransactionRequest,
    selector: StateSelector,
    overrides: VirtualExecutionOverrides,
  ) -> Result<Simulation<'_>, RuntimeSimulationError> {
    let request = SimulationRequest::new(request)?;
    let view = self.epoch_for_simulation(selector)?;
    Ok(Simulation::new(
      self.chain_spec.machine().as_ref(),
      view,
      request,
      overrides,
    )?)
  }

  pub(crate) fn espace_call(
    &self,
    request: TransactionRequest,
    selector: StateSelector,
    overrides: VirtualExecutionOverrides,
  ) -> Result<Vec<u8>, RuntimeSimulationError> {
    Ok(
      self
        .espace_simulation(request, selector, overrides)?
        .call()?,
    )
  }

  pub(crate) fn espace_estimate_gas(
    &self,
    request: TransactionRequest,
    selector: StateSelector,
    overrides: VirtualExecutionOverrides,
  ) -> Result<TransactionEstimate, RuntimeSimulationError> {
    Ok(
      self
        .espace_simulation(request, selector, overrides)?
        .estimate()?,
    )
  }

  pub(crate) fn espace_create_access_list(
    &self,
    request: TransactionRequest,
    selector: StateSelector,
    overrides: VirtualExecutionOverrides,
  ) -> Result<AccessListResult, RuntimeSimulationError> {
    Ok(
      self
        .espace_simulation(request, selector, overrides)?
        .create_access_list()?,
    )
  }
}
