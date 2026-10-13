use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::value::RawValue;

use super::response::{RpcError, RpcResponse};

pub(super) enum Request {
  Single(Call),
  Batch(Vec<Call>),
}

pub(super) enum Call {
  Method(MethodCall),
  Invalid { id: Id },
}

#[derive(Deserialize)]
pub(super) struct MethodCall {
  #[serde(rename = "jsonrpc")]
  _version: Version,
  pub(super) method: String,
  // Method lookup precedes parameter validation, including parameter shape.
  #[serde(default, deserialize_with = "present")]
  pub(super) params: Option<Box<RawValue>>,
  /// None denotes a notification; Some(Id::Null) still requires a response.
  #[serde(default, deserialize_with = "present")]
  pub(super) id: Option<Id>,
}

struct Version;

impl<'de> Deserialize<'de> for Version {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let version = String::deserialize(deserializer)?;
    if version == "2.0" {
      Ok(Self)
    } else {
      Err(de::Error::custom("expected JSON-RPC version 2.0"))
    }
  }
}

#[derive(Default, Serialize)]
#[serde(untagged)]
pub(super) enum Id {
  #[default]
  Null,
  String(String),
  Number(Box<RawValue>),
}

impl Request {
  pub(super) fn parse(input: &[u8]) -> Result<Self, RpcResponse> {
    let raw: &RawValue = serde_json::from_slice(input)
      .map_err(|_| RpcResponse::new(Id::Null, Err(RpcError::parse_error())))?;

    if raw.get().starts_with('[') {
      let entries: Vec<&RawValue> = serde_json::from_str(raw.get())
        .expect("a validated JSON array contains valid raw JSON values");
      if entries.is_empty() {
        return Err(RpcResponse::new(Id::Null, Err(RpcError::invalid_request())));
      }
      Ok(Self::Batch(entries.into_iter().map(Call::parse).collect()))
    } else {
      Ok(Self::Single(Call::parse(raw)))
    }
  }
}

impl Call {
  fn parse(raw: &RawValue) -> Self {
    // Serde structs also accept sequences; a JSON-RPC call must be an object.
    if !raw.get().starts_with('{') {
      return Self::Invalid { id: Id::Null };
    }
    match serde_json::from_str(raw.get()) {
      Ok(call) => Self::Method(call),
      Err(_) => {
        // Recover only the response identity, never a partially valid call.
        // A missing, invalid, or duplicated id cannot identify the response.
        #[derive(Deserialize)]
        struct InvalidCall {
          #[serde(default)]
          id: Id,
        }

        let id = serde_json::from_str::<InvalidCall>(raw.get())
          .map(|call| call.id)
          .unwrap_or_default();
        Self::Invalid { id }
      }
    }
  }
}

impl<'de> Deserialize<'de> for Id {
  fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
    let raw = Box::<RawValue>::deserialize(deserializer)?;
    match raw.get().as_bytes().first().copied() {
      Some(b'n') => Ok(Self::Null),
      Some(b'"') => serde_json::from_str(raw.get())
        .map(Self::String)
        .map_err(|_| de::Error::custom("invalid JSON-RPC string id")),
      Some(b'-' | b'0'..=b'9') => Ok(Self::Number(raw)),
      _ => Err(de::Error::custom(
        "JSON-RPC id must be a string, number, or null",
      )),
    }
  }
}

// Option's normal deserialization treats null as absence. Here only an omitted
// field is absent; null must reach the id or method-specific parameter parser.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
  D: Deserializer<'de>,
  T: Deserialize<'de>,
{
  T::deserialize(deserializer).map(Some)
}
