//! Converts captured eSpace query results without consulting the current Runtime.

use cfx_rpc_eth_types::{Bytes, Header, Log, LogData, Receipt, Transaction as RpcTransaction};
use cfx_types::{
  AddressSpaceUtil, CreateContractAddressType, H256, Space, U64, U256,
  cal_contract_address_with_space,
};
use primitives::{
  Action, Transaction, TransactionStatus, TransactionWithSignature,
  transaction::eth_transaction::eip155_signature,
};
use rlp::Encodable;
use serde::Serialize;
use serde_json::Value;

use crate::{
  chain::{EspaceBlockView, EspaceTransactionRef, EspaceTransactionView},
  fork::ForkReadError,
  node::{EspaceBlock, EspaceBlockReceipts, EspaceReceipt, EspaceTransaction},
  rpc::{operation::RpcResult, response::RpcError},
  rpc_client::EthereumBlock,
  runtime::EspaceStateQueryError,
  runtime_transaction::RuntimeTransaction,
};

fn value<T: Serialize>(value: T) -> RpcResult {
  serde_json::to_value(value).map_err(|_| RpcError::internal())
}

pub(super) fn state<T: Serialize>(result: Result<T, EspaceStateQueryError>) -> RpcResult {
  value(result.map_err(state_error)?)
}

pub(super) fn code(result: Result<std::sync::Arc<Vec<u8>>, EspaceStateQueryError>) -> RpcResult {
  let code = result.map_err(state_error)?;
  value(Bytes::new(code.as_ref().clone()))
}

pub(super) fn block(result: Result<Option<EspaceBlock>, ForkReadError>, full: bool) -> RpcResult {
  match result.map_err(fork_error)? {
    None => Ok(Value::Null),
    Some(EspaceBlock::Local(block)) => value(local_block(&block, full)),
    Some(EspaceBlock::Fork(block)) => value(block.as_ref()),
  }
}

pub(super) fn block_transaction_count(
  result: Result<Option<EspaceBlock>, ForkReadError>,
) -> RpcResult {
  let count = result.map_err(fork_error)?.map(|block| {
    U256::from(match block {
      EspaceBlock::Local(block) => block.transactions().len(),
      EspaceBlock::Fork(block) => block.transaction_count(),
    })
  });
  value(count)
}

pub(super) fn transaction(result: Result<Option<EspaceTransaction>, ForkReadError>) -> RpcResult {
  transaction_value(result.map_err(fork_error)?)
}

pub(super) fn transaction_value(transaction: Option<EspaceTransaction>) -> RpcResult {
  match transaction {
    None => Ok(Value::Null),
    Some(EspaceTransaction::Pool(transaction)) => value(runtime_transaction(
      &transaction,
      (None, None, None),
      (None, None),
    )),
    Some(EspaceTransaction::Local(transaction)) => value(local_transaction(&transaction)),
    Some(EspaceTransaction::Fork(transaction)) => value(transaction.as_ref()),
  }
}

pub(super) fn raw_transaction(
  result: Result<Option<EspaceTransaction>, ForkReadError>,
) -> RpcResult {
  let raw = match result.map_err(fork_error)? {
    None => return Ok(Value::Null),
    Some(EspaceTransaction::Pool(transaction)) => transaction.query_raw(),
    Some(EspaceTransaction::Local(transaction)) => query_raw(transaction.transaction()),
    Some(EspaceTransaction::Fork(transaction)) => {
      let raw = transaction
        .raw
        .as_ref()
        .ok_or_else(|| fork_error(ForkReadError::Unavailable("raw eSpace transaction")))?;
      let decoded = TransactionWithSignature::from_raw(&raw.0).map_err(|_| {
        fork_error(ForkReadError::Inconsistent(
          "raw eSpace transaction encoding",
        ))
      })?;
      // Phantom signatures establish an identity, not a recovered user sender.
      if decoded.space() != Space::Ethereum
        || !decoded.is_canonical_rlp()
        || decoded.hash() != transaction.hash
      {
        return Err(fork_error(ForkReadError::Inconsistent(
          "raw eSpace transaction identity",
        )));
      }
      raw.0.clone()
    }
  };
  value(Bytes::new(raw))
}

pub(super) fn receipt(result: Result<Option<EspaceReceipt>, ForkReadError>) -> RpcResult {
  match result.map_err(fork_error)? {
    None => Ok(Value::Null),
    Some(EspaceReceipt::Fork(receipt)) => value(receipt.as_ref()),
    Some(EspaceReceipt::Local(transaction)) => value(local_receipt(&transaction)),
  }
}

pub(super) fn block_receipts(
  result: Result<Option<EspaceBlockReceipts>, ForkReadError>,
) -> RpcResult {
  match result
    .map_err(fork_error)?
    .ok_or_else(RpcError::invalid_params)?
  {
    EspaceBlockReceipts::Fork(receipts) => value(receipts.as_ref()),
    EspaceBlockReceipts::Local(block) => {
      let receipts: Vec<_> = block
        .transactions()
        .map(|transaction| local_receipt(&transaction))
        .collect();
      value(receipts)
    }
  }
}

fn local_block(block: &EspaceBlockView, full: bool) -> EthereumBlock {
  let pivot = block.pivot_header();
  let empty = block.transactions().len() == 0;
  let empty_root = keccak_hash::keccak([0x80]);
  let size: usize = block
    .transactions()
    .map(|transaction| match transaction.transaction() {
      EspaceTransactionRef::Phantom(transaction) => transaction.rlp_size(),
      EspaceTransactionRef::Original(transaction) => match transaction.fork_transaction() {
        Some(transaction) => transaction.rlp_size(),
        None => {
          let raw = transaction.query_raw();
          if transaction.transaction().is_2718() {
            rlp::encode(&raw).len()
          } else {
            raw.len()
          }
        }
      },
    })
    .sum();
  let header = Header {
    hash: block.hash(),
    parent_hash: *pivot.parent_hash(),
    uncles_hash: keccak_hash::keccak([0xc0]),
    author: *pivot.author(),
    miner: *pivot.author(),
    state_root: *pivot.deferred_state_root(),
    transactions_root: if empty {
      empty_root
    } else {
      *pivot.transactions_root()
    },
    receipts_root: if empty {
      empty_root
    } else {
      *pivot.deferred_receipts_root()
    },
    number: block.epoch_height().into(),
    gas_used: block.gas_used(),
    gas_limit: pivot.espace_gas_limit(true),
    espace_gas_limit: block.total_gas_limit(),
    extra_data: Bytes::default(),
    logs_bloom: *block.logs_bloom(),
    timestamp: pivot.timestamp().into(),
    difficulty: *pivot.difficulty(),
    total_difficulty: U256::zero(),
    base_fee_per_gas: pivot.base_price().map(|price| price[Space::Ethereum]),
    size: size.into(),
    nonce: pivot.nonce().low_u64().to_be_bytes().into(),
    mix_hash: H256::zero(),
  };
  if full {
    EthereumBlock::from_transactions(
      header,
      block
        .transactions()
        .map(|transaction| local_transaction(&transaction))
        .collect(),
    )
  } else {
    EthereumBlock::from_hashes(
      header,
      block
        .transactions()
        .map(|transaction| transaction.hash())
        .collect(),
    )
  }
}

fn local_transaction(view: &EspaceTransactionView) -> RpcTransaction {
  let transaction = view.transaction();
  let receipt = view.receipt();
  let block_info = (
    Some(view.block().hash()),
    Some(view.block().epoch_height().into()),
    Some(view.transaction_index().into()),
  );
  let exec_info = (
    Some(receipt.outcome_status().in_space(Space::Ethereum).into()),
    created_address(transaction, receipt.outcome_status()),
  );
  match transaction {
    EspaceTransactionRef::Original(transaction) => {
      runtime_transaction(transaction, block_info, exec_info)
    }
    EspaceTransactionRef::Phantom(transaction) => {
      RpcTransaction::from_signed(transaction, block_info, exec_info)
    }
  }
}

fn runtime_transaction(
  transaction: &RuntimeTransaction,
  block_info: (Option<H256>, Option<U64>, Option<U256>),
  exec_info: (Option<U64>, Option<cfx_types::Address>),
) -> RpcTransaction {
  if let Some(signed) = transaction.fork_transaction() {
    return RpcTransaction::from_signed(signed, block_info, exec_info);
  }
  let unsigned = transaction.transaction();
  let v = if unsigned.is_2718() {
    0
  } else {
    eip155_signature::add_chain_replay_protection(0, unsigned.chain_id().map(u64::from))
  };
  RpcTransaction {
    transaction_type: Some(unsigned.type_id().into()),
    hash: transaction.hash(),
    nonce: *unsigned.nonce(),
    block_hash: block_info.0,
    block_number: block_info.1,
    transaction_index: block_info.2,
    from: transaction.sender(),
    to: match unsigned.action() {
      Action::Create => None,
      Action::Call(address) => Some(address),
    },
    value: *unsigned.value(),
    gas_price: Some(*unsigned.gas_price()),
    max_fee_per_gas: unsigned.after_1559().then_some(*unsigned.gas_price()),
    max_priority_fee_per_gas: unsigned
      .after_1559()
      .then_some(*unsigned.max_priority_gas_price()),
    gas: *unsigned.gas(),
    input: Bytes::new(unsigned.data().clone()),
    creates: exec_info.1,
    raw: Some(Bytes::new(transaction.query_raw())),
    public_key: None,
    chain_id: unsigned.chain_id().map(|id| u64::from(id).into()),
    standard_v: Some(U256::zero()),
    v: v.into(),
    r: U256::one(),
    s: U256::one(),
    status: exec_info.0,
    access_list: unsigned.access_list().cloned(),
    y_parity: unsigned.is_2718().then_some(U64::zero()),
    authorization_list: unsigned
      .authorization_list()
      .map(|list| list.iter().cloned().map(Into::into).collect()),
  }
}

fn query_raw(transaction: EspaceTransactionRef<'_>) -> Vec<u8> {
  match transaction {
    EspaceTransactionRef::Original(transaction) => transaction.query_raw(),
    EspaceTransactionRef::Phantom(transaction) => {
      transaction.transaction.transaction.rlp_bytes().to_vec()
    }
  }
}

fn created_address(
  transaction: EspaceTransactionRef<'_>,
  status: TransactionStatus,
) -> Option<cfx_types::Address> {
  if status != TransactionStatus::Success || transaction.transaction().action() != Action::Create {
    return None;
  }
  let unsigned = transaction.transaction();
  Some(
    cal_contract_address_with_space(
      CreateContractAddressType::FromSenderNonce,
      &transaction.sender().with_evm_space(),
      unsigned.nonce(),
      unsigned.data(),
    )
    .0
    .address,
  )
}

fn local_receipt(view: &EspaceTransactionView) -> Receipt {
  let transaction = view.transaction();
  let unsigned: &Transaction = transaction.transaction();
  let receipt = view.receipt();
  let block = view.block();
  let index: U256 = view.transaction_index().into();
  let logs: Vec<_> = receipt
    .logs()
    .iter()
    .enumerate()
    .map(|(transaction_log_index, log)| Log {
      inner: LogData {
        address: log.address,
        topics: log.topics.clone(),
        data: Bytes::new(log.data.clone()),
      },
      block_hash: block.hash(),
      block_number: block.epoch_height().into(),
      transaction_hash: transaction.hash(),
      transaction_index: index,
      log_index: Some((view.first_log_index() + transaction_log_index).into()),
      transaction_log_index: Some(transaction_log_index.into()),
      block_timestamp: Some(block.pivot_header().timestamp().into()),
      removed: false,
    })
    .collect();
  let effective_gas_price = match block.pivot_header().base_price() {
    Some(prices) if *unsigned.gas_price() >= prices[Space::Ethereum] => {
      unsigned.effective_gas_price(&prices[Space::Ethereum])
    }
    _ => *unsigned.gas_price(),
  };
  Receipt {
    transaction_type: receipt.burnt_gas_fee().map(|_| unsigned.type_id().into()),
    transaction_hash: transaction.hash(),
    transaction_index: index,
    block_hash: block.hash(),
    from: transaction.sender(),
    to: match unsigned.action() {
      Action::Create => None,
      Action::Call(address) => Some(address),
    },
    block_number: block.epoch_height().into(),
    cumulative_gas_used: receipt.accumulated_gas_used(),
    gas_used: view.gas_used(),
    gas_fee: receipt.gas_fee(),
    contract_address: created_address(transaction, receipt.outcome_status()),
    logs,
    logs_bloom: *receipt.logs_bloom(),
    status_code: receipt.outcome_status().in_space(Space::Ethereum).into(),
    effective_gas_price,
    tx_exec_error_msg: receipt.execution_error_message().map(str::to_owned),
    burnt_gas_fee: receipt.burnt_gas_fee(),
  }
}

fn state_error(error: EspaceStateQueryError) -> RpcError {
  match error {
    EspaceStateQueryError::Unavailable(_) | EspaceStateQueryError::UnknownBlock(_) => {
      RpcError::invalid_params()
    }
    EspaceStateQueryError::Fork(error) => fork_error(error),
    EspaceStateQueryError::State(_) => processing_error("State query failed"),
  }
}

fn fork_error(error: ForkReadError) -> RpcError {
  match error {
    ForkReadError::Closed => RpcError {
      code: -32000,
      message: "Node is closed".into(),
      data: None,
    },
    ForkReadError::ServiceStopped => RpcError::internal(),
    _ => processing_error("Remote query failed"),
  }
}

fn processing_error(message: &str) -> RpcError {
  // Conflux maps state/storage processing failures to EXCEPTION_ERROR.
  RpcError {
    code: -32016,
    message: message.into(),
    data: None,
  }
}
