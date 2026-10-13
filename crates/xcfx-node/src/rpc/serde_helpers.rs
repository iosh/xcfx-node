//! Serde adapters for method names and positional JSON-RPC parameters.

use std::{fmt, marker::PhantomData};

use serde::{
  Deserialize, Deserializer,
  de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IgnoredAny, SeqAccess, VariantAccess,
    Visitor,
    value::{BorrowedStrDeserializer, EnumAccessDeserializer},
  },
};
use serde_json::value::RawValue;
use thiserror::Error;

use super::response::RpcError;

/// Decodes an externally tagged request enum from a validated JSON-RPC envelope.
/// An unknown method returns None without inspecting its parameters.
/// Unit and tuple variants describe positional signatures; newtype variants use
/// deserialize_single or deserialize_optional to handle the outer parameter array.
pub(super) fn deserialize_request<T: DeserializeOwned>(
  method: &str,
  params: Option<&RawValue>,
) -> Result<Option<T>, RpcError> {
  let access = MethodAccess {
    method,
    params: params.map_or("[]", RawValue::get),
  };
  match T::deserialize(EnumAccessDeserializer::new(access)) {
    Ok(request) => Ok(Some(request)),
    Err(DecodeError::UnknownMethod) => Ok(None),
    Err(DecodeError::InvalidParams) => Err(RpcError::invalid_params()),
  }
}

/// Decodes a single positional argument for a Serde newtype variant.
pub(super) fn deserialize_single<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
  D: Deserializer<'de>,
  T: Deserialize<'de>,
{
  let [value] = <[T; 1]>::deserialize(deserializer)?;
  Ok(value)
}

/// Accepts an empty array or one parameter; an explicit null element means None.
pub(super) fn deserialize_optional<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
  D: Deserializer<'de>,
  T: Deserialize<'de>,
{
  struct OptionalParamVisitor<T>(PhantomData<T>);

  impl<'de, T: Deserialize<'de>> Visitor<'de> for OptionalParamVisitor<T> {
    type Value = Option<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
      formatter.write_str("an array with at most one parameter")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
      let value = seq.next_element::<Option<T>>()?.flatten();
      if seq.next_element::<IgnoredAny>()?.is_some() {
        return Err(de::Error::invalid_length(2, &self));
      }
      Ok(value)
    }
  }

  deserializer.deserialize_seq(OptionalParamVisitor(PhantomData))
}

#[derive(Debug, Error)]
enum DecodeError {
  #[error("unknown RPC method")]
  UnknownMethod,
  #[error("invalid RPC parameters")]
  InvalidParams,
}

impl de::Error for DecodeError {
  fn custom<T: fmt::Display>(_message: T) -> Self {
    Self::InvalidParams
  }
}

struct MethodAccess<'a> {
  method: &'a str,
  params: &'a str,
}

impl<'de> EnumAccess<'de> for MethodAccess<'de> {
  type Error = DecodeError;
  type Variant = ParamsAccess<'de>;

  fn variant_seed<V: DeserializeSeed<'de>>(
    self,
    seed: V,
  ) -> Result<(V::Value, Self::Variant), Self::Error> {
    // The derived enum selects the method using its rename and alias attributes.
    // Keep this failure separate from errors in nested parameter enums.
    let variant = seed
      .deserialize(BorrowedStrDeserializer::<DecodeError>::new(self.method))
      .map_err(|_| DecodeError::UnknownMethod)?;
    if !self.params.starts_with('[') {
      return Err(DecodeError::InvalidParams);
    }
    Ok((variant, ParamsAccess(self.params)))
  }
}

struct ParamsAccess<'a>(&'a str);

impl<'de> VariantAccess<'de> for ParamsAccess<'de> {
  type Error = DecodeError;

  fn unit_variant(self) -> Result<(), Self::Error> {
    serde_json::from_str::<[(); 0]>(self.0)
      .map(|_| ())
      .map_err(|_| DecodeError::InvalidParams)
  }

  fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Self::Error> {
    seed
      .deserialize(&mut serde_json::Deserializer::from_str(self.0))
      .map_err(|_| DecodeError::InvalidParams)
  }

  fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error> {
    serde_json::Deserializer::from_str(self.0)
      .deserialize_tuple(len, visitor)
      .map_err(|_| DecodeError::InvalidParams)
  }

  fn struct_variant<V: Visitor<'de>>(
    self,
    fields: &'static [&'static str],
    visitor: V,
  ) -> Result<V::Value, Self::Error> {
    self.tuple_variant(fields.len(), visitor)
  }
}
