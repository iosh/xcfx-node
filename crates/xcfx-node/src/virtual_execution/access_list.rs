use std::collections::BTreeSet;

use cfx_executor::executive::TxDropError;
use cfx_types::{Space, U256};
use primitives::{
  AccessList, Action, SignedTransaction, Transaction,
  transaction::{Eip2930Transaction, EthereumTransaction, TransactionError},
};

use crate::runtime_transaction::fake_sign_for_execution;

use super::{
  AccessListObserver, AccessListResult, EstimatedOutcome, Simulation, SimulationContext,
  SimulationFailure, VirtualExecutionError,
};

impl SimulationContext<'_> {
  pub(super) fn access_list_observer(&self, transaction: &SignedTransaction) -> AccessListObserver {
    let mut excluded = BTreeSet::from([transaction.sender]);
    let destination = match transaction.action() {
      Action::Call(address) => address,
      Action::Create => {
        transaction
          .cal_created_address()
          .expect("a validated create transaction has a creation address")
          .address
      }
    };
    excluded.insert(destination);
    let builtins = match transaction.space() {
      Space::Native => self.machine.builtins(),
      Space::Ethereum => self.machine.builtins_evm(),
    };
    excluded.extend(builtins.keys().copied());
    AccessListObserver::new(transaction.access_list().unwrap_or(&Vec::new()), excluded)
  }
}

impl Simulation<'_> {
  pub(crate) fn create_access_list(mut self) -> Result<AccessListResult, VirtualExecutionError> {
    let access_list = match self.context.estimate(&self.transaction, true)? {
      EstimatedOutcome::NotExecuted(failure) => return Err(failure.into()),
      EstimatedOutcome::Failed {
        failure,
        gas_used,
        access_list,
      } => {
        return Ok(AccessListResult {
          access_list: access_list
            .expect("access-list collection preserves observations on failure"),
          gas_used,
          error: Some(failure),
        });
      }
      EstimatedOutcome::Finished(result) => result
        .access_list
        .expect("access-list collection produces a list on success"),
    };
    let transaction = transaction_with_access_list(self.transaction, access_list.clone());
    match self.context.validate(&transaction) {
      Ok(()) => {}
      Err(TransactionError::NotEnoughBaseGas { required, got }) => {
        return Ok(AccessListResult {
          access_list,
          gas_used: U256::zero(),
          error: Some(SimulationFailure::Drop(TxDropError::NotEnoughGasLimit {
            expected: required,
            got,
          })),
        });
      }
      Err(error) => return Err(error.into()),
    }
    let (gas_used, error) = match self.context.estimate(&transaction, false)? {
      EstimatedOutcome::Finished(result) => (result.protocol_gas_used, None),
      EstimatedOutcome::Failed {
        failure, gas_used, ..
      } => (gas_used, Some(failure)),
      EstimatedOutcome::NotExecuted(SimulationFailure::Drop(
        error @ TxDropError::NotEnoughGasLimit { .. },
      )) => (U256::zero(), Some(SimulationFailure::Drop(error))),
      EstimatedOutcome::NotExecuted(failure) => return Err(failure.into()),
    };
    Ok(AccessListResult {
      access_list,
      gas_used,
      error,
    })
  }
}

fn transaction_with_access_list(
  transaction: SignedTransaction,
  access_list: AccessList,
) -> SignedTransaction {
  let sender = transaction.sender;
  let unsigned = transaction.transaction.transaction.unsigned;
  let Transaction::Ethereum(unsigned) = unsigned else {
    unreachable!("only eSpace RPC exposes access-list generation")
  };
  let unsigned = match unsigned {
    EthereumTransaction::Eip155(transaction) => EthereumTransaction::Eip2930(Eip2930Transaction {
      chain_id: transaction
        .chain_id
        .expect("request preparation resolves the legacy chain ID"),
      nonce: transaction.nonce,
      gas_price: transaction.gas_price,
      gas: transaction.gas,
      action: transaction.action,
      value: transaction.value,
      data: transaction.data,
      access_list,
    }),
    EthereumTransaction::Eip2930(mut transaction) => {
      transaction.access_list = access_list;
      EthereumTransaction::Eip2930(transaction)
    }
    EthereumTransaction::Eip1559(mut transaction) => {
      transaction.access_list = access_list;
      EthereumTransaction::Eip1559(transaction)
    }
    EthereumTransaction::Eip7702(mut transaction) => {
      transaction.access_list = access_list;
      EthereumTransaction::Eip7702(transaction)
    }
  };
  fake_sign_for_execution(Transaction::Ethereum(unsigned), sender)
}
