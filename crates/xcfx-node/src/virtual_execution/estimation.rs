//! Conflux gas and collateral estimation, including sponsor and balance checks.

use cfx_executor::executive::{ChargeCollateral, ExecutionError};
use cfx_parameters::staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT;
use cfx_types::{Address, U256, U512, address_util::AddressUtil};
use cfx_vm_types::Error as VmError;
use primitives::{Action, SignedTransaction, Transaction};

use super::{
  EstimatedExecution, EstimatedOutcome, ObservedExecution, ObservedOutcome, SimulationContext,
  TransactionEstimate, VirtualExecutionError,
};

enum SponsorResource {
  Gas,
  Collateral,
}

impl SimulationContext<'_> {
  fn sponsored_contract(
    &self,
    transaction: &SignedTransaction,
    resource: SponsorResource,
  ) -> Result<Option<Address>, VirtualExecutionError> {
    if let Transaction::Native(native) = &transaction.unsigned
      && let Action::Call(address) = native.action()
      && address.is_contract_address()
    {
      let sponsor = match resource {
        SponsorResource::Collateral => self.state.sponsor_for_collateral(&address)?,
        SponsorResource::Gas => self.state.sponsor_for_gas(&address)?,
      };
      if sponsor.is_some_and(|address| !address.is_zero())
        && self
          .state
          .check_contract_whitelist(&address, &transaction.sender)?
      {
        return Ok(Some(address));
      }
    }
    Ok(None)
  }

  fn select_collateral_result(
    &mut self,
    transaction: &SignedTransaction,
    sender_result: ObservedExecution,
  ) -> Result<(ObservedExecution, Option<u64>), VirtualExecutionError> {
    let Some(contract) = self.sponsored_contract(transaction, SponsorResource::Collateral)? else {
      return Ok((sender_result, None));
    };
    let sponsor_result = self.transact(transaction, ChargeCollateral::EstimateSponsor, None)?;
    let capacity = (U512::from(self.state.sponsor_balance_for_collateral(&contract)?)
      + U512::from(
        self
          .state
          .available_storage_points_for_collateral(&contract)?,
      ))
      / U512::from(*DRIPS_PER_STORAGE_COLLATERAL_UNIT);
    if let ObservedOutcome::Finished(sponsor_result) = sponsor_result
      && capacity >= U512::from(storage_limit(&sponsor_result))
    {
      return Ok((sponsor_result, None));
    }
    // Only the sender fallback needs to exceed the sponsor's capacity. A
    // large valid sponsor budget does not itself require a u64 conversion.
    let required = (capacity + U512::from(64)).max(U512::from(storage_limit(&sender_result)));
    if required > U512::from(u64::MAX) {
      return Err(VirtualExecutionError::StorageLimitOutOfRange { required });
    }
    Ok((sender_result, Some(required.low_u64())))
  }

  pub(super) fn estimate(
    &mut self,
    transaction: &SignedTransaction,
    collect_access_list: bool,
  ) -> Result<EstimatedOutcome, VirtualExecutionError> {
    let access_list = collect_access_list.then(|| self.access_list_observer(transaction));
    let sender_result =
      match self.transact(transaction, ChargeCollateral::EstimateSender, access_list)? {
        ObservedOutcome::NotExecuted(failure) => return Ok(EstimatedOutcome::NotExecuted(failure)),
        ObservedOutcome::Failed(error, executed) => return Ok(executed.fail(error)),
        ObservedOutcome::Finished(executed) => executed,
      };
    let (mut selected, storage_override) =
      self.select_collateral_result(transaction, sender_result)?;
    let gas_limit = estimated_gas_limit(&selected, transaction, self.spec.align_evm);
    let required_storage = storage_override.unwrap_or_else(|| storage_limit(&selected));
    let protocol_gas_used = selected.executed.gas_used;
    let fee = if self.options.has_gas() {
      U512::from(selected.executed.fee)
    } else {
      gas_limit.full_mul(*transaction.gas_price())
    };

    if self.options.has_price() && selected.executed.gas_sponsor_paid {
      selected.executed.gas_sponsor_paid =
        match self.sponsored_contract(transaction, SponsorResource::Gas)? {
          None => false,
          Some(contract) if !self.options.charge_gas() => {
            fee <= U512::from(self.state.sponsor_balance_for_gas(&contract)?)
              && fee <= U512::from(self.state.sponsor_gas_bound(&contract)?)
          }
          Some(_) => true,
        };
    }
    if self.options.has_sender() {
      let gas_fee = if self.options.recheck_fee() && !selected.executed.gas_sponsor_paid {
        gas_limit.full_mul(*transaction.gas_price())
      } else {
        U512::zero()
      };
      let storage_cost = if selected.executed.storage_sponsor_paid {
        U256::zero()
      } else {
        U256::from(required_storage) * *DRIPS_PER_STORAGE_COLLATERAL_UNIT
      };
      let required = U512::from(*transaction.value()) + gas_fee + U512::from(storage_cost);
      let balance = self.state.balance(&transaction.sender())?;
      if U512::from(balance) < required {
        let actual_gas_cost = U512::from(balance)
          .min(gas_fee)
          .try_into()
          .expect("actual gas cost is bounded by the sender's U256 balance");
        return Ok(selected.fail(ExecutionError::NotEnoughCash {
          required,
          got: balance.into(),
          actual_gas_cost,
          max_storage_limit_cost: storage_cost,
        }));
      }
    }
    if transaction
      .storage_limit()
      .is_some_and(|limit| limit < required_storage)
    {
      return Ok(selected.fail(ExecutionError::VmError(VmError::ExceedStorageLimit)));
    }
    Ok(EstimatedOutcome::Finished(EstimatedExecution {
      estimate: TransactionEstimate {
        gas_limit,
        storage_limit: required_storage,
      },
      output: selected.executed.output,
      protocol_gas_used,
      access_list: selected.access_list,
    }))
  }
}

fn storage_limit(result: &ObservedExecution) -> u64 {
  result
    .executed
    .storage_collateralized
    .first()
    .map_or(0, |change| change.collaterals.as_u64())
}

fn estimated_gas_limit(
  result: &ObservedExecution,
  transaction: &SignedTransaction,
  align_evm: bool,
) -> U256 {
  let data = transaction.data();
  let zero_bytes = data.iter().filter(|byte| **byte == 0).count();
  let floor =
    U256::from(21_000) + U256::from(zero_bytes) * 10 + U256::from(data.len() - zero_bytes) * 40;
  let calldata_floor = if align_evm {
    U256::zero()
  } else {
    U256::from(data.len()) * 100
  };
  let estimated: U256 = result.gas_required * 7 / 6 + result.executed.base_gas;
  estimated.max(calldata_floor).max(floor)
}
