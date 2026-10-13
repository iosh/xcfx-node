use alloy_rpc_types_eth::BlockNumberOrTag;
use cfx_rpc_eth_types::{Bytes, Index};
use cfx_types::{H64, H256, U256};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime::EpochSelector;

use super::{operation::Operation, serde_helpers};

#[derive(Deserialize)]
pub(super) enum EspaceRequest {
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
        json!({"eth":"1.0", "net":"1.0", "web3":"1.0", "rpc":"1.0", "xcfx":"1.0", "anvil":"1.0", "evm":"1.0", "hardhat":"1.0"})
      }
    };
    Operation::Immediate(value)
  }
}
