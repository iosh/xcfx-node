//! Receives eSpace history while preserving Conflux field widths.

use cfx_rpc_eth_types::{Bytes, Header, Log, Receipt, Transaction};
use cfx_types::{Bloom, H64, H160, H256, U64, U256};
use serde::{Deserialize, Serialize};

// Upstream BlockTransactions is not exported, so the block envelope is local.
#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct EthereumBlock {
  #[serde(flatten, deserialize_with = "HeaderFields::deserialize")]
  pub(crate) header: Header,
  transactions: BlockTransactions,
  uncles: Vec<H256>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum BlockTransactions {
  Hashes(Vec<H256>),
  Full(Vec<Transaction>),
}

#[derive(Debug, Deserialize)]
#[serde(transparent)]
pub(super) struct EthereumReceiptResponse(
  #[serde(deserialize_with = "ReceiptFields::deserialize")] pub(super) Receipt,
);

// Upstream Header and Receipt serialize but do not implement Deserialize.
#[derive(Deserialize)]
#[serde(remote = "Header", rename_all = "camelCase")]
struct HeaderFields {
  hash: H256,
  parent_hash: H256,
  #[serde(rename = "sha3Uncles")]
  uncles_hash: H256,
  author: H160,
  miner: H160,
  state_root: H256,
  transactions_root: H256,
  receipts_root: H256,
  number: U64,
  gas_used: U256,
  gas_limit: U256,
  espace_gas_limit: U256,
  extra_data: Bytes,
  logs_bloom: Bloom,
  timestamp: U256,
  difficulty: U256,
  total_difficulty: U256,
  base_fee_per_gas: Option<U256>,
  size: U256,
  nonce: H64,
  mix_hash: H256,
}

#[derive(Deserialize)]
#[serde(remote = "Receipt", rename_all = "camelCase")]
struct ReceiptFields {
  #[serde(rename = "type")]
  transaction_type: Option<U64>,
  transaction_hash: H256,
  transaction_index: U256,
  block_hash: H256,
  from: H160,
  to: Option<H160>,
  block_number: U64,
  cumulative_gas_used: U256,
  gas_used: U256,
  gas_fee: U256,
  contract_address: Option<H160>,
  logs: Vec<Log>,
  logs_bloom: Bloom,
  #[serde(rename = "status")]
  status_code: U64,
  effective_gas_price: U256,
  tx_exec_error_msg: Option<String>,
  burnt_gas_fee: Option<U256>,
}
