//! Node transaction preparation and admission share one effective-state view.

use cfx_types::{AddressSpaceUtil, AddressWithSpace, H256, Space, U256};
use primitives::{Transaction, transaction::TransactionError};
use thiserror::Error;

use crate::{
  fork::ForkReadError,
  runtime_transaction::RuntimeTransaction,
  transaction_ingress::{TransactionValidationContext, decode_and_validate_raw_transaction},
  transaction_request::{
    EthereumTransactionType, RequestSpace, TransactionRequest,
    build::{TransactionFees, TransactionFields, TransactionTemplate, TransactionType},
  },
  virtual_execution::{Simulation, VirtualExecutionError},
};

use super::NodeRuntime;

#[derive(Debug, Error)]
pub(crate) enum RuntimeTransactionError {
  #[error(transparent)]
  Fork(#[from] ForkReadError),
  #[error(transparent)]
  Simulation(#[from] VirtualExecutionError),
  #[error(transparent)]
  Transaction(#[from] TransactionError),
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),
  #[error("no signing key is configured for address {address:?}")]
  SigningKeyNotFound { address: AddressWithSpace },
  #[error("no signing accounts are configured for {space:?}")]
  NoSigningAccounts { space: Space },
  #[error("raw transaction uses {got:?}; this endpoint requires {expected:?}")]
  RawSpaceMismatch { expected: Space, got: Space },
}

impl NodeRuntime {
  fn node_transaction_sender(
    &self,
    request: &TransactionRequest,
  ) -> Result<AddressWithSpace, RuntimeTransactionError> {
    if let Some(sender) = request.from() {
      return Ok(sender);
    }
    let space = request.space();
    self
      .signing_keys
      .addresses(space)
      .next()
      .map(|address| address.with_space(space))
      .ok_or(RuntimeTransactionError::NoSigningAccounts { space })
  }

  fn submission_fees(
    &self,
    request: &TransactionRequest,
    transaction_type: TransactionType,
  ) -> Result<TransactionFees, ForkReadError> {
    let suggested_price = || match request.space() {
      Space::Native => self.core_gas_price(),
      Space::Ethereum => Ok(self.espace_gas_price()),
    };

    if !transaction_type.has_dynamic_fee() {
      let gas_price = match request.gas_price {
        Some(price) => price,
        None => suggested_price()?,
      };

      return Ok(TransactionFees {
        gas_price,
        priority_fee: U256::zero(),
      });
    }
    let priority_fee = request
      .max_priority_fee_per_gas
      .unwrap_or_else(|| match request.space() {
        Space::Native => U256::zero(),
        Space::Ethereum => U256::from(1_000_000_000u64),
      });
    let gas_price = match request.max_fee_per_gas {
      Some(price) => price,
      None => {
        let price = suggested_price()?;

        match request.space() {
          Space::Native => price.max(priority_fee),
          Space::Ethereum => price,
        }
      }
    };

    Ok(TransactionFees {
      gas_price,
      priority_fee,
    })
  }

  fn prepare_node_transaction(
    &self,
    sender: AddressWithSpace,
    request: TransactionRequest,
  ) -> Result<Transaction, RuntimeTransactionError> {
    let transaction_type = submission_type(&request);
    let fees = self.submission_fees(&request, transaction_type)?;
    let view = self.runtime_state.history.optimistic_head();
    let nonce = match request.nonce {
      Some(nonce) => nonce,
      None => self.pending_nonce(sender)?,
    };
    let epoch_height = match request.space {
      RequestSpace::Native { epoch_height, .. } => epoch_height.unwrap_or(view.epoch_height()),
      RequestSpace::Ethereum { .. } => view.epoch_height(),
    };
    let chain_id = request.chain_id.unwrap_or_else(|| {
      self
        .chain_spec
        .machine()
        .params()
        .chain_id(view.epoch_height(), request.space())
    });
    let template = TransactionTemplate::new(
      &request,
      transaction_type,
      TransactionFields {
        sender,
        nonce,
        chain_id,
        epoch_height,
        fees,
      },
    );
    let (gas, storage_limit) = match (&request.space, request.gas) {
      (RequestSpace::Ethereum { .. }, Some(gas)) => (gas, 0),
      (
        RequestSpace::Native {
          storage_limit: Some(storage_limit),
          ..
        },
        Some(gas),
      ) => (gas, *storage_limit),
      _ => {
        let estimate = Simulation::for_estimation(
          self.chain_spec.machine().as_ref(),
          view,
          self.runtime_state.effective_state.state(),
          &template,
        )?
        .estimate()?;
        (
          request.gas.unwrap_or(estimate.gas_limit),
          request.storage_limit().unwrap_or(estimate.storage_limit),
        )
      }
    };
    Ok(template.build(gas, storage_limit))
  }

  /// Returns signed wire bytes without pool insertion, nonce reservation, or mining.
  pub(crate) fn sign_transaction_request(
    &self,
    request: TransactionRequest,
  ) -> Result<Vec<u8>, RuntimeTransactionError> {
    let sender = self.node_transaction_sender(&request)?;
    let signer = self
      .signing_keys
      .transaction_signer(sender)
      .ok_or(RuntimeTransactionError::SigningKeyNotFound { address: sender })?;
    let transaction = self.prepare_node_transaction(sender, request)?;
    Ok(RuntimeTransaction::from_recovered_signature(signer.sign(transaction)).query_raw())
  }

  /// Chooses key signing before impersonation, then admits only the completed transaction.
  pub(crate) fn submit_transaction_request(
    &mut self,
    request: TransactionRequest,
  ) -> Result<H256, RuntimeTransactionError> {
    let sender = self.node_transaction_sender(&request)?;
    let signer = self.signing_keys.transaction_signer(sender);
    if signer.is_none() && !self.impersonation.is_authorized(sender) {
      return Err(RuntimeTransactionError::SigningKeyNotFound { address: sender });
    }
    let transaction = self.prepare_node_transaction(sender, request)?;
    let transaction = match signer {
      Some(signer) => RuntimeTransaction::from_recovered_signature(signer.sign(transaction)),
      None => RuntimeTransaction::from_impersonated(transaction, sender.address),
    };
    self.with_current_transaction_validation(|validation| {
      validation.validate_runtime_for_pool_admission(&transaction)
    })?;
    self.admit_transaction(transaction)
  }

  /// Decodes and validates external wire bytes before admitting them to this Space.
  pub(crate) fn submit_raw_transaction(
    &mut self,
    raw: &[u8],
    space: Space,
  ) -> Result<H256, RuntimeTransactionError> {
    let transaction = self.decode_raw_transaction(raw)?;
    if transaction.space() != space {
      return Err(RuntimeTransactionError::RawSpaceMismatch {
        expected: space,
        got: transaction.space(),
      });
    }
    self.admit_transaction(transaction)
  }

  /// Core's raw RPC accepts both protocol Spaces, as the upstream shared pool does.
  pub(crate) fn submit_core_raw_transaction(
    &mut self,
    raw: &[u8],
  ) -> Result<H256, RuntimeTransactionError> {
    let transaction = self.decode_raw_transaction(raw)?;
    self.admit_transaction(transaction)
  }

  fn decode_raw_transaction(
    &self,
    raw: &[u8],
  ) -> Result<RuntimeTransaction, RuntimeTransactionError> {
    Ok(self.with_current_transaction_validation(|validation| {
      decode_and_validate_raw_transaction(raw, validation)
    })?)
  }

  fn with_current_transaction_validation<T>(
    &self,
    callback: impl FnOnce(&TransactionValidationContext<'_>) -> T,
  ) -> T {
    let view = self.runtime_state.history.optimistic_head();
    let epoch_height = view.epoch_height();
    let params = self.chain_spec.machine().params();
    let spec = self
      .chain_spec
      .machine()
      .spec(view.pivot_block_number(), epoch_height);
    callback(&TransactionValidationContext::new(
      params,
      &spec,
      epoch_height,
    ))
  }

  /// Callers in this module establish identity and validate protocol rules once.
  /// Duplicate, effective nonce, replacement and capacity checks precede pool mutation.
  fn admit_transaction(
    &mut self,
    transaction: RuntimeTransaction,
  ) -> Result<H256, RuntimeTransactionError> {
    let hash = transaction.hash();
    if self.runtime_state.history.contains_mined_transaction(&hash) {
      return Err(TransactionError::AlreadyImported.into());
    }
    let state_nonce = self
      .open_effective_state_for_reading()?
      .nonce(&transaction.sender_with_space())?;
    if *transaction.nonce() < state_nonce {
      return Err(TransactionError::Stale.into());
    }
    self.runtime_state.transaction_pool.insert(transaction)?;
    Ok(hash)
  }
}

fn submission_type(request: &TransactionRequest) -> TransactionType {
  if let RequestSpace::Native {
    transaction_type, ..
  } = request.space
  {
    return TransactionType::Native(transaction_type);
  }
  let transaction_type = if request.authorization_list().is_some() {
    EthereumTransactionType::Authorization
  } else if request.gas_price.is_some() && request.access_list.is_some() {
    EthereumTransactionType::AccessList
  } else if request.gas_price.is_some() {
    EthereumTransactionType::Legacy
  } else {
    EthereumTransactionType::DynamicFee
  };
  TransactionType::Ethereum(transaction_type)
}
