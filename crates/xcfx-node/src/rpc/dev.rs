use std::time::Duration;

use serde::{Deserialize, Deserializer, de};
use serde_json::{Value, json};

use crate::{
  config::MiningMode,
  node::{MineRequest, MineTiming, MiningError, MiningOutcome},
  runtime::RuntimeBlockProductionError,
};

use super::{operation::Operation, response::RpcError, serde_helpers};

#[derive(Deserialize)]
pub(super) enum DevRequest {
  #[serde(rename = "xcfx_mine", alias = "anvil_mine", alias = "hardhat_mine")]
  Mine(
    /// Number of blocks; defaults to one.
    #[serde(default)]
    Option<NumericU64>,
    /// Logical timestamp increment in seconds, scoped to this request.
    #[serde(default)]
    Option<NumericU64>,
  ),
  #[serde(
    rename = "evm_mine",
    deserialize_with = "serde_helpers::deserialize_optional"
  )]
  EvmMine(Option<EvmMineInput>),
  #[serde(
    rename = "xcfx_setAutomine",
    alias = "anvil_setAutomine",
    alias = "evm_setAutomine",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  SetAutomine(bool),
  #[serde(
    rename = "xcfx_getAutomine",
    alias = "anvil_getAutomine",
    alias = "hardhat_getAutomine"
  )]
  GetAutomine,
  #[serde(
    rename = "xcfx_setIntervalMining",
    alias = "anvil_setIntervalMining",
    alias = "evm_setIntervalMining",
    deserialize_with = "serde_helpers::deserialize_single"
  )]
  SetIntervalMining(NumericU64),
  #[serde(rename = "xcfx_getIntervalMining", alias = "anvil_getIntervalMining")]
  GetIntervalMining,
}

impl DevRequest {
  pub(super) fn into_operation(self) -> Operation {
    match self {
      Self::Mine(blocks, interval) => {
        let blocks = blocks.map_or(1, |value| value.0);
        let timing = interval.map_or(MineTiming::Default, |value| MineTiming::Interval(value.0));
        Operation::on_node(
          move |node| node.mine(MineRequest { blocks, timing }),
          |outcome| mining_result(outcome).map(|()| Value::Null),
        )
      }
      Self::EvmMine(input) => {
        let (blocks, timestamp) = match input {
          None => (1, None),
          Some(EvmMineInput::Timestamp(timestamp)) => (1, Some(timestamp.0)),
          Some(EvmMineInput::Options { timestamp, blocks }) => {
            (blocks.unwrap_or(1), timestamp.map(|value| value.0))
          }
        };
        let timing = timestamp.map_or(MineTiming::Default, MineTiming::Timestamp);
        Operation::on_node(
          move |node| node.mine(MineRequest { blocks, timing }),
          |outcome| mining_result(outcome).map(|()| json!("0x0")),
        )
      }
      Self::SetAutomine(enabled) => {
        Operation::on_node(move |node| node.set_automine(enabled), |_| Ok(Value::Null))
      }
      Self::GetAutomine => Operation::on_node(
        |node| node.mining_mode(),
        |mode| Ok(json!(mode == MiningMode::Auto)),
      ),
      Self::SetIntervalMining(interval) => Operation::on_node(
        move |node| node.set_interval_mining(Duration::from_secs(interval.0)),
        |result| {
          result
            .map(|_| Value::Null)
            .map_err(|_| RpcError::invalid_params())
        },
      ),
      Self::GetIntervalMining => Operation::on_node(
        |node| node.mining_mode(),
        |mode| {
          Ok(match mode {
            MiningMode::Interval(period) => json!(period.as_secs()),
            _ => Value::Null,
          })
        },
      ),
    }
  }
}

fn mining_result(outcome: MiningOutcome) -> Result<(), RpcError> {
  match outcome {
    MiningOutcome::Complete(_) => Ok(()),
    MiningOutcome::Interrupted { hashes, reason } => {
      let mut error = mining_error(&reason);
      error.data = Some(json!({"hashes": hashes}));
      Err(error)
    }
  }
}

fn mining_error(error: &MiningError) -> RpcError {
  match error {
    MiningError::Request(error) => RpcError::from(error),
    MiningError::Interval(_) | MiningError::Production(RuntimeBlockProductionError::Time(_)) => {
      RpcError::invalid_params()
    }
    MiningError::Production(_) => RpcError {
      code: -32000,
      message: "Block production failed".into(),
      data: None,
    },
  }
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(super) enum EvmMineInput {
  Options {
    timestamp: Option<NumericU64>,
    blocks: Option<u64>,
  },
  Timestamp(NumericU64),
}

/// Development controls accept JSON integers and hex/decimal strings, without truncation.
pub(super) struct NumericU64(u64);

impl<'de> Deserialize<'de> for NumericU64 {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
      Integer(u64),
      String(String),
    }
    match Input::deserialize(deserializer)? {
      Input::Integer(value) => Ok(Self(value)),
      Input::String(value) => {
        let (digits, radix) = value
          .strip_prefix("0x")
          .map_or((value.as_str(), 10), |s| (s, 16));
        if digits.is_empty()
          || !digits.bytes().all(|b| {
            if radix == 16 {
              b.is_ascii_hexdigit()
            } else {
              b.is_ascii_digit()
            }
          })
        {
          return Err(de::Error::custom("expected an unsigned quantity"));
        }
        u64::from_str_radix(digits, radix)
          .map(Self)
          .map_err(|_| de::Error::custom("quantity exceeds u64"))
      }
    }
  }
}
