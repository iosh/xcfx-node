//! Core wire projections of already captured blocks, transactions, and receipts.

use std::sync::Arc;

use cfx_addr::Network;
use cfx_rpc_cfx_types::{
  Block, BlockTransactions, Header, Log, Receipt, RpcAddress, Transaction,
  access_list::from_primitive_access_list, receipt::StorageChange,
};
use cfx_types::{
  Address, CreateContractAddressType, H256, Space, U256, address_util::AddressUtil,
  cal_contract_address,
};
use primitives::{Action, BlockHeader, TransactionStatus};

use crate::{
  block_producer::RuntimeBlock,
  chain::{MinedBlockView, MinedTransactionView, TransactionReceiptView},
  node::{CoreBlock, CoreDebugTransactions, CoreEpochReceipts, CoreReceipt, CoreTransaction},
  runtime_transaction::RuntimeTransaction,
};

pub(in crate::rpc) fn address(value: Address, network: Network) -> RpcAddress {
  RpcAddress::try_from_h160(value, network)
    .expect("a validated instance network can encode an address")
}

pub(in crate::rpc) fn header(
  block: &RuntimeBlock,
  epoch: Option<u64>,
  number: Option<u64>,
  network: Network,
) -> Header {
  let header = block.header();
  Header {
    hash: block.hash(),
    parent_hash: *header.parent_hash(),
    height: header.height().into(),
    miner: address(*header.author(), network),
    deferred_state_root: *header.deferred_state_root(),
    deferred_receipts_root: *header.deferred_receipts_root(),
    deferred_logs_bloom_hash: *header.deferred_logs_bloom_hash(),
    blame: header.blame().into(),
    transactions_root: *header.transactions_root(),
    epoch_number: epoch.map(Into::into),
    block_number: number.map(Into::into),
    gas_limit: *header.gas_limit(),
    base_fee_per_gas: header.base_price().map(|prices| prices[Space::Native]),
    timestamp: header.timestamp().into(),
    difficulty: *header.difficulty(),
    pow_quality: header.pow_hash.map(|hash| pow_quality(header, hash)),
    referee_hashes: header.referee_hashes().clone(),
    adaptive: header.adaptive(),
    nonce: header.nonce(),
    custom: header.custom_items().into_iter().map(Into::into).collect(),
    pos_reference: *header.pos_reference(),
  }
}

fn pow_quality(header: &BlockHeader, hash: H256) -> U256 {
  // Conflux measures distance from the nonce's high 127 bits, modulo 2^256.
  let lower = ((header.nonce() >> 128) << 128) & (U256::MAX >> 1);
  let distance = U256::from_big_endian(hash.as_bytes())
    .overflowing_sub(lower)
    .0;
  if distance == U256::MAX {
    return U256::one();
  }
  let boundary = distance + U256::one();
  if boundary == U256::one() {
    return U256::MAX;
  }
  // floor(2^256 / boundary), without representing 2^256 in U256.
  U256::MAX / boundary + U256::from(u8::from(U256::MAX % boundary == boundary - U256::one()))
}

pub(super) fn block(value: CoreBlock, network: Network, full: bool) -> Block {
  match value {
    CoreBlock::Local(view) => local_block(view.runtime_block(), Some(&view), network, full),
    CoreBlock::Detached(block) => local_block(&block, None, network, full),
    CoreBlock::Fork(block) => {
      let mut block = Arc::unwrap_or_clone(block);
      block.miner = address(block.miner.hex_address, network);
      if let BlockTransactions::Full(transactions) = &mut block.transactions {
        for transaction in transactions {
          normalize_transaction(transaction, network);
        }
      }
      block
    }
  }
}

fn local_block(
  block: &RuntimeBlock,
  view: Option<&MinedBlockView>,
  network: Network,
  full: bool,
) -> Block {
  let h = header(
    block,
    view.map(|v| v.epoch_height()),
    view.and_then(|v| v.block_number()),
    network,
  );
  let gas_used = view.and_then(MinedBlockView::core_gas_used);
  let transactions = if full {
    BlockTransactions::Full(
      block
        .transactions()
        .iter()
        .enumerate()
        .filter(|(_, tx)| tx.space() == Space::Native)
        .map(|(index, tx)| {
          let mut result = pending_transaction(tx, network);
          if let Some(view) = view.and_then(|v| v.transaction_at(index))
            && let Some(receipt) = view.receipt()
            && receipt.receipt().outcome_status != TransactionStatus::Skipped
          {
            apply_position(
              &mut result,
              &view,
              Some(receipt.receipt().outcome_status),
              network,
            );
          }
          result
        })
        .collect(),
    )
  } else {
    BlockTransactions::Hashes(
      block
        .transactions()
        .iter()
        .filter(|tx| tx.space() == Space::Native)
        .map(RuntimeTransaction::hash)
        .collect(),
    )
  };
  Block {
    hash: h.hash,
    parent_hash: h.parent_hash,
    height: h.height,
    miner: h.miner,
    deferred_state_root: h.deferred_state_root,
    deferred_receipts_root: h.deferred_receipts_root,
    deferred_logs_bloom_hash: h.deferred_logs_bloom_hash,
    blame: h.blame,
    transactions_root: h.transactions_root,
    epoch_number: h.epoch_number,
    block_number: h.block_number,
    gas_limit: block.header().core_space_gas_limit(),
    gas_used,
    base_fee_per_gas: h.base_fee_per_gas,
    timestamp: h.timestamp,
    difficulty: h.difficulty,
    pow_quality: h.pow_quality,
    referee_hashes: h.referee_hashes,
    adaptive: h.adaptive,
    nonce: h.nonce,
    transactions,
    size: block.standard_block().map(|block| block.size().into()),
    custom: h.custom,
    pos_reference: h.pos_reference,
  }
}

pub(in crate::rpc) fn pending_transaction(
  transaction: &RuntimeTransaction,
  network: Network,
) -> Transaction {
  if let Some(signed) = transaction.fork_transaction() {
    return Transaction::from_signed(signed, None, network)
      .expect("a retained transaction has encodable addresses");
  }
  let unsigned = transaction.transaction();
  let epoch_height = match unsigned {
    primitives::Transaction::Native(native) => *native.epoch_height(),
    primitives::Transaction::Ethereum(_) => 0,
  };
  Transaction {
    transaction_type: Some(unsigned.type_id().into()),
    space: (transaction.space() == Space::Ethereum).then_some(Space::Ethereum),
    hash: transaction.hash(),
    nonce: *transaction.nonce(),
    block_hash: None,
    transaction_index: None,
    from: address(transaction.sender(), network),
    to: match unsigned.action() {
      Action::Create => None,
      Action::Call(to) => Some(address(to, network)),
    },
    value: *transaction.value(),
    gas_price: *transaction.gas_price(),
    gas: *transaction.gas(),
    contract_created: None,
    data: unsigned.data().clone().into(),
    storage_limit: unsigned.storage_limit().unwrap_or_default().into(),
    epoch_height: epoch_height.into(),
    chain_id: unsigned.chain_id().map(Into::into),
    status: None,
    access_list: unsigned
      .access_list()
      .cloned()
      .map(|list| from_primitive_access_list(list, network)),
    max_priority_fee_per_gas: unsigned
      .after_1559()
      .then_some(*unsigned.max_priority_gas_price()),
    max_fee_per_gas: unsigned.after_1559().then_some(*unsigned.gas_price()),
    v: U256::zero(),
    r: U256::one(),
    s: U256::one(),
    y_parity: unsigned.is_2718().then_some(0.into()),
  }
}

fn created_address(
  transaction: &RuntimeTransaction,
  status: TransactionStatus,
  network: Network,
) -> Option<RpcAddress> {
  if status != TransactionStatus::Success || transaction.transaction().action() != Action::Create {
    return None;
  }
  let kind = match transaction.space() {
    Space::Native => CreateContractAddressType::FromSenderNonceAndCodeHash,
    Space::Ethereum => CreateContractAddressType::FromSenderNonce,
  };
  let (mut created, _) = cal_contract_address(
    kind,
    &transaction.sender(),
    transaction.nonce(),
    transaction.transaction().data(),
  );
  if transaction.space() == Space::Native {
    created.set_contract_type_bits();
  }
  Some(address(created, network))
}

fn apply_position(
  value: &mut Transaction,
  view: &MinedTransactionView,
  status: Option<TransactionStatus>,
  network: Network,
) {
  value.block_hash = Some(view.block().hash());
  value.transaction_index = Some(
    view
      .core_transaction_index()
      .expect("a Core transaction has a Native index")
      .into(),
  );
  value.status = status.map(|status| status.in_space(Space::Native).into());
  value.contract_created =
    status.and_then(|status| created_address(view.transaction(), status, network));
}

pub(super) fn transaction(value: CoreTransaction, network: Network) -> Transaction {
  match value {
    CoreTransaction::Pool(transaction) => pending_transaction(&transaction, network),
    CoreTransaction::Local(view) => {
      let mut value = pending_transaction(view.transaction(), network);
      let status = view
        .receipt()
        .map(|receipt| receipt.receipt().outcome_status);
      apply_position(&mut value, &view, status, network);
      value
    }
    CoreTransaction::Fork(value) => {
      let mut value = Arc::unwrap_or_clone(value);
      normalize_transaction(&mut value, network);
      value
    }
  }
}

fn normalize_transaction(value: &mut Transaction, network: Network) {
  value.from = address(value.from.hex_address, network);
  value.to = value.to.as_ref().map(|a| address(a.hex_address, network));
  value.contract_created = value
    .contract_created
    .as_ref()
    .map(|a| address(a.hex_address, network));
  if let Some(list) = &mut value.access_list {
    for item in list {
      item.address = address(item.address.hex_address, network);
    }
  }
}

pub(super) fn receipt(value: CoreReceipt, network: Network) -> Receipt {
  match value {
    CoreReceipt::Local(view) => local_receipt(
      &view,
      network,
      false,
      view
        .transaction()
        .core_transaction_index()
        .expect("a Core receipt has a Native index"),
    ),
    CoreReceipt::Fork(value) => {
      let mut value = Arc::unwrap_or_clone(value);
      normalize_receipt(&mut value, network);
      value
    }
  }
}

fn local_receipt(
  view: &TransactionReceiptView,
  network: Network,
  include_espace: bool,
  index: usize,
) -> Receipt {
  let transaction = view.transaction().transaction();
  let unsigned = transaction.transaction();
  let block = view.transaction().block();
  let result = view.receipt();
  let effective_gas_price = match block.epoch_base_price() {
    Some(prices) if *unsigned.gas_price() >= prices[transaction.space()] => {
      unsigned.effective_gas_price(&prices[transaction.space()])
    }
    _ => *unsigned.gas_price(),
  };
  Receipt {
    transaction_type: Some(unsigned.type_id().into()),
    transaction_hash: transaction.hash(),
    index: index.into(),
    block_hash: block.hash(),
    epoch_number: Some(block.epoch_height().into()),
    from: address(transaction.sender(), network),
    to: match unsigned.action() {
      Action::Create => None,
      Action::Call(to) => Some(address(to, network)),
    },
    gas_used: view.gas_used(),
    accumulated_gas_used: Some(result.accumulated_gas_used),
    gas_fee: result.gas_fee,
    effective_gas_price,
    contract_created: created_address(transaction, result.outcome_status, network),
    logs: result
      .logs
      .iter()
      .filter(|log| include_espace || log.space == Space::Native)
      .map(|log| {
        Log::try_from(log.clone(), network, include_espace)
          .expect("a retained log has an encodable address")
      })
      .collect(),
    logs_bloom: result.log_bloom,
    state_root: block.epoch_state_root().unwrap_or_default(),
    outcome_status: result.outcome_status.in_space(transaction.space()).into(),
    tx_exec_error_msg: view.execution_error_message().map(str::to_owned),
    gas_covered_by_sponsor: result.gas_sponsor_paid,
    storage_covered_by_sponsor: result.storage_sponsor_paid,
    storage_collateralized: result
      .storage_collateralized
      .first()
      .map(|change| change.collaterals)
      .unwrap_or_default(),
    storage_released: result
      .storage_released
      .iter()
      .map(|change| StorageChange {
        address: address(change.address, network),
        collaterals: change.collaterals,
      })
      .collect(),
    space: include_espace.then_some(transaction.space()),
    burnt_gas_fee: result.burnt_gas_fee,
  }
}

pub(super) fn epoch_receipts(
  value: CoreEpochReceipts,
  network: Network,
  include_espace: bool,
) -> Vec<Vec<Receipt>> {
  match value {
    CoreEpochReceipts::Local(blocks) => blocks
      .into_iter()
      .map(|block| {
        block
          .transactions()
          .iter()
          .enumerate()
          .filter(|(_, tx)| include_espace || tx.space() == Space::Native)
          .enumerate()
          .filter_map(|(index, (original, _))| {
            block
              .transaction_at(original)
              .and_then(|tx| tx.receipt())
              .map(|receipt| local_receipt(&receipt, network, include_espace, index))
          })
          .collect()
      })
      .collect(),
    CoreEpochReceipts::Fork(value) => {
      let mut receipts = value.receipts.clone();
      for group in &mut receipts {
        for receipt in group {
          normalize_receipt(receipt, network);
        }
      }
      receipts
    }
  }
}

fn normalize_receipt(value: &mut Receipt, network: Network) {
  value.from = address(value.from.hex_address, network);
  value.to = value.to.as_ref().map(|a| address(a.hex_address, network));
  value.contract_created = value
    .contract_created
    .as_ref()
    .map(|a| address(a.hex_address, network));
  for log in &mut value.logs {
    log.address = address(log.address.hex_address, network);
  }
  for change in &mut value.storage_released {
    change.address = address(change.address.hex_address, network);
  }
}

pub(super) fn debug_transactions(
  value: CoreDebugTransactions,
  network: Network,
) -> Vec<cfx_rpc_eth_types::WrapTransaction> {
  use cfx_rpc_eth_types::WrapTransaction;

  let blocks = match value {
    CoreDebugTransactions::Local(blocks) => blocks,
    CoreDebugTransactions::Fork(transactions) => {
      return transactions
        .iter()
        .cloned()
        .map(|mut transaction| {
          if let WrapTransaction::NativeTransaction(transaction) = &mut transaction {
            normalize_transaction(transaction, network);
          }

          transaction
        })
        .collect();
    }
  };

  let mut transactions = Vec::new();

  for block in blocks {
    let mut espace_index = 0usize;

    for (index, transaction) in block.transactions().iter().enumerate() {
      let view = block
        .transaction_at(index)
        .expect("an enumerated block transaction has a view");
      let status = view
        .receipt()
        .map(|receipt| receipt.receipt().outcome_status);

      match transaction.space() {
        Space::Native => {
          let mut value = pending_transaction(transaction, network);

          if let Some(status) = status
            && status != TransactionStatus::Skipped
          {
            apply_position(&mut value, &view, Some(status), network);
          }

          transactions.push(WrapTransaction::NativeTransaction(value));
        }

        Space::Ethereum => {
          let block_info = (
            Some(block.epoch_id()),
            Some(block.epoch_height().into()),
            Some(espace_index.into()),
          );
          let exec_info = (
            status.map(|status| status.in_space(Space::Ethereum).into()),
            status
              .and_then(|status| created_address(transaction, status, network))
              .map(|address| address.hex_address),
          );

          let value =
            crate::rpc::espace::convert::runtime_transaction(transaction, block_info, exec_info);

          transactions.push(WrapTransaction::EthTransaction(value));
          espace_index += 1;
        }
      }
    }
  }

  transactions
}
