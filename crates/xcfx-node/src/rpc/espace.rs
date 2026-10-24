mod convert;

use alloy_rpc_types_eth::BlockNumberOrTag;
use cfx_rpc_eth_types::{BlockId, Bytes, Index};
use cfx_types::{Address, BigEndianHash, H64, H256, U256};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime::{EpochSelector, EspaceBlockSelector, EspaceStateSelector};

use super::{operation::Operation, serde_helpers};

#[derive(Deserialize)]
pub(super) enum EspaceRequest {
  #[serde(rename = "eth_getBalance")]
  Balance(Address, #[serde(default)] Option<BlockId>),
  #[serde(rename = "eth_getCode")]
  Code(Address, #[serde(default)] Option<BlockId>),
  #[serde(rename = "eth_getStorageAt")]
  Storage(Address, U256, #[serde(default)] Option<BlockId>),
  #[serde(rename = "eth_getTransactionCount")]
  TransactionCount(Address, #[serde(default)] Option<BlockId>),
  #[serde(rename = "eth_getBlockByHash")]
  BlockByHash(H256, bool),
  #[serde(rename = "eth_getBlockByNumber")]
  BlockByNumber(BlockNumberOrTag, bool),
  #[serde(
    rename = "eth_getBlockTransactionCountByHash",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  BlockTransactionCountByHash(H256),
  #[serde(
    rename = "eth_getBlockTransactionCountByNumber",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  BlockTransactionCountByNumber(BlockNumberOrTag),
  #[serde(
    rename = "eth_getTransactionByHash",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  TransactionByHash(H256),
  #[serde(rename = "eth_getTransactionByBlockHashAndIndex")]
  TransactionByBlockHashAndIndex(H256, Index),
  #[serde(rename = "eth_getTransactionByBlockNumberAndIndex")]
  TransactionByBlockNumberAndIndex(BlockNumberOrTag, Index),
  #[serde(
    rename = "eth_getTransactionReceipt",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  TransactionReceipt(H256),
  #[serde(
    rename = "eth_getBlockReceipts",
    alias = "parity_getBlockReceipts",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  BlockReceipts(BlockId),
  #[serde(
    rename = "eth_getRawTransactionByHash",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  RawTransactionByHash(H256),
  #[serde(rename = "eth_getRawTransactionByBlockHashAndIndex")]
  RawTransactionByBlockHashAndIndex(H256, Index),
  #[serde(rename = "eth_getRawTransactionByBlockNumberAndIndex")]
  RawTransactionByBlockNumberAndIndex(BlockNumberOrTag, Index),
  #[serde(rename = "eth_getTransactionBySenderAndNonce")]
  TransactionBySenderAndNonce(Address, TransactionNonce),
  #[serde(rename = "eth_chainId")]
  ChainId,
  #[serde(rename = "eth_blockNumber")]
  BlockNumber,
  #[serde(rename = "eth_syncing")]
  Syncing,
  #[serde(rename = "net_version")]
  NetVersion,
  #[serde(rename = "net_listening")]
  Listening,
  #[serde(rename = "net_peerCount")]
  PeerCount,
  #[serde(rename = "web3_clientVersion")]
  ClientVersion,
  #[serde(rename = "eth_protocolVersion")]
  ProtocolVersion,
  #[serde(rename = "eth_coinbase")]
  Coinbase,
  #[serde(rename = "eth_mining")]
  Mining,
  #[serde(rename = "eth_hashrate")]
  Hashrate,
  #[serde(rename = "eth_submitWork")]
  SubmitWork(H64, H256, H256),
  #[serde(rename = "eth_submitHashrate")]
  SubmitHashrate(U256, H256),
  #[serde(
    rename = "eth_getUncleCountByBlockHash",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  UncleCountByHash(H256),
  #[serde(
    rename = "eth_getUncleCountByBlockNumber",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  UncleCountByNumber(BlockNumberOrTag),
  #[serde(rename = "eth_getUncleByBlockHashAndIndex")]
  UncleByHash(H256, Index),
  #[serde(rename = "eth_getUncleByBlockNumberAndIndex")]
  UncleByNumber(BlockNumberOrTag, Index),
  #[serde(
    rename = "web3_sha3",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  Sha3(Bytes),
  #[serde(rename = "rpc_modules")]
  Modules,
}

impl EspaceRequest {
  pub(super) fn into_operation(self) -> Operation {
    let value = match self {
      Self::Balance(address, block) => {
        let selector = state_selector(block.unwrap_or_default(), true);
        return Operation::on_node(
          move |node| node.espace_balance_at(address, selector),
          convert::state,
        );
      }
      Self::Code(address, block) => {
        let selector = state_selector(block.unwrap_or_default(), true);
        return Operation::on_node(
          move |node| node.espace_code_at(address, selector),
          convert::code,
        );
      }
      Self::Storage(address, position, block) => {
        let selector = state_selector(block.unwrap_or_default(), true);
        let position = H256::from_uint(&position);
        return Operation::on_node(
          move |node| node.espace_storage_at(address, position, selector),
          convert::state,
        );
      }
      Self::TransactionCount(address, block) => {
        let block = block.unwrap_or_default();
        if block == BlockId::Pending {
          return Operation::on_node(
            move |node| node.espace_pending_nonce(address),
            convert::state,
          );
        }
        let selector = state_selector(block, false);
        return Operation::on_node(
          move |node| node.espace_nonce_at(address, selector),
          convert::state,
        );
      }
      Self::BlockByHash(hash, full) => {
        return Operation::on_node(
          move |node| node.espace_block(EspaceBlockSelector::Hash(hash), full),
          move |result| convert::block(result, full),
        );
      }
      Self::BlockByNumber(number, full) => {
        let selector = EspaceBlockSelector::Epoch(number_selector(number));
        return Operation::on_node(
          move |node| node.espace_block(selector, full),
          move |result| convert::block(result, full),
        );
      }
      Self::BlockTransactionCountByHash(hash) => {
        return Operation::on_node(
          move |node| node.espace_block(EspaceBlockSelector::Hash(hash), false),
          convert::block_transaction_count,
        );
      }
      Self::BlockTransactionCountByNumber(number) => {
        let selector = EspaceBlockSelector::Epoch(number_selector(number));
        return Operation::on_node(
          move |node| node.espace_block(selector, false),
          convert::block_transaction_count,
        );
      }
      Self::TransactionByHash(hash) => {
        return Operation::on_node(
          move |node| node.espace_transaction_by_hash(hash),
          convert::transaction,
        );
      }
      Self::TransactionByBlockHashAndIndex(hash, index) => {
        return Operation::on_node(
          move |node| {
            node
              .espace_transaction_by_block_and_index(EspaceBlockSelector::Hash(hash), index.value())
          },
          convert::transaction,
        );
      }
      Self::TransactionByBlockNumberAndIndex(number, index) => {
        let selector = EspaceBlockSelector::Epoch(number_selector(number));
        return Operation::on_node(
          move |node| node.espace_transaction_by_block_and_index(selector, index.value()),
          convert::transaction,
        );
      }
      Self::TransactionReceipt(hash) => {
        return Operation::on_node(
          move |node| node.espace_transaction_receipt_by_hash(&hash),
          convert::receipt,
        );
      }
      Self::BlockReceipts(block) => {
        let selector = block_selector(block);
        return Operation::on_node(
          move |node| node.espace_block_receipts(selector),
          convert::block_receipts,
        );
      }
      Self::RawTransactionByHash(hash) => {
        return Operation::on_node(
          move |node| node.espace_transaction_by_hash(hash),
          convert::raw_transaction,
        );
      }
      Self::RawTransactionByBlockHashAndIndex(hash, index) => {
        return Operation::on_node(
          move |node| {
            node
              .espace_transaction_by_block_and_index(EspaceBlockSelector::Hash(hash), index.value())
          },
          convert::raw_transaction,
        );
      }
      Self::RawTransactionByBlockNumberAndIndex(number, index) => {
        let selector = EspaceBlockSelector::Epoch(number_selector(number));
        return Operation::on_node(
          move |node| node.espace_transaction_by_block_and_index(selector, index.value()),
          convert::raw_transaction,
        );
      }
      Self::TransactionBySenderAndNonce(address, nonce) => {
        let nonce = match nonce {
          TransactionNonce::Quantity(nonce) => nonce,
          TransactionNonce::Number(nonce) => nonce.into(),
        };
        return Operation::on_node(
          move |node| node.espace_transaction_by_sender_and_nonce(address, nonce),
          convert::transaction_value,
        );
      }
      Self::ChainId => {
        return Operation::on_node(
          |node| node.espace_chain_id(),
          |id| Ok(json!(format!("0x{id:x}"))),
        );
      }
      Self::NetVersion => {
        return Operation::on_node(
          |node| node.espace_chain_id(),
          |id| Ok(json!(id.to_string())),
        );
      }
      Self::BlockNumber => {
        return Operation::on_node(
          |node| node.epoch_height(EpochSelector::LatestState),
          |height| Ok(json!(format!("0x{height:x}"))),
        );
      }
      Self::SubmitWork(_nonce, _pow_hash, _mix_digest) => json!(false),
      Self::SubmitHashrate(_hashrate, _id) => json!(false),
      Self::UncleCountByHash(_hash) => json!("0x0"),
      Self::UncleCountByNumber(_number) => json!("0x0"),
      Self::UncleByHash(_hash, _index) => Value::Null,
      Self::UncleByNumber(_number, _index) => Value::Null,
      Self::Sha3(bytes) => json!(keccak_hash::keccak(&bytes.0)),
      Self::Syncing | Self::Mining => json!(false),
      Self::Listening => json!(true),
      Self::PeerCount | Self::Hashrate => json!("0x0"),
      Self::ClientVersion => json!(format!(
        "xcfx-node/v{}/{}",
        crate::build_info().version,
        crate::build_info().target
      )),
      Self::ProtocolVersion => json!("0x41"),
      Self::Coinbase => json!("0x0000000000000000000000000000000000000000"),
      Self::Modules => {
        json!({"eth":"1.0", "net":"1.0", "web3":"1.0", "parity":"1.0", "rpc":"1.0", "xcfx":"1.0", "anvil":"1.0", "evm":"1.0", "hardhat":"1.0"})
      }
    };
    Operation::Immediate(value)
  }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum TransactionNonce {
  Quantity(U256),
  Number(u64),
}

fn number_selector(number: BlockNumberOrTag) -> EpochSelector {
  match number {
    BlockNumberOrTag::Number(number) => EpochSelector::Number(number),
    BlockNumberOrTag::Earliest => EpochSelector::Earliest,
    BlockNumberOrTag::Latest | BlockNumberOrTag::Pending => EpochSelector::LatestState,
    BlockNumberOrTag::Safe => EpochSelector::Confirmed,
    BlockNumberOrTag::Finalized => EpochSelector::Finalized,
  }
}

fn epoch_selector(block: BlockId) -> EpochSelector {
  match block {
    BlockId::Num(number) => EpochSelector::Number(number),
    BlockId::Earliest => EpochSelector::Earliest,
    BlockId::Latest | BlockId::Pending => EpochSelector::LatestState,
    BlockId::Safe => EpochSelector::Confirmed,
    BlockId::Finalized => EpochSelector::Finalized,
    BlockId::Hash { .. } => unreachable!("a hash selector must be handled before epoch conversion"),
  }
}

fn state_selector(block: BlockId, require_pivot: bool) -> EspaceStateSelector {
  match block {
    BlockId::Hash {
      hash,
      require_canonical,
    } => EspaceStateSelector::Hash {
      hash,
      require_pivot: require_pivot || require_canonical.unwrap_or(true),
    },
    block => EspaceStateSelector::Epoch(epoch_selector(block)),
  }
}

fn block_selector(block: BlockId) -> EspaceBlockSelector {
  match block {
    BlockId::Hash { hash, .. } => EspaceBlockSelector::Hash(hash),
    block => EspaceBlockSelector::Epoch(epoch_selector(block)),
  }
}
