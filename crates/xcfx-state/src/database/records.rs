//! Storage values carry their collateral attribution through every state layer.
use cfx_types::{Address, U256};
use std::sync::Arc;

/// Attribution of a nonzero storage entry. Unknown is an inherited payment,
/// not an address or an instruction to charge a new payer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StorageOwner {
  #[default]
  None,
  Address(Address),
  Unknown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StorageEntry {
  pub(crate) value: U256,
  pub(crate) owner: StorageOwner,
}

impl StorageEntry {
  pub fn new(value: U256, owner: Option<Address>) -> Self {
    Self::with_owner(value, owner.map(StorageOwner::Address).unwrap_or_default())
  }
  pub fn with_unknown_owner(value: U256) -> Self {
    Self::with_owner(value, StorageOwner::Unknown)
  }
  pub(crate) fn with_owner(value: U256, owner: StorageOwner) -> Self {
    Self {
      value,
      owner: if value.is_zero() {
        StorageOwner::None
      } else {
        owner
      },
    }
  }
  pub fn value(&self) -> U256 {
    self.value
  }
  pub fn owner(&self) -> StorageOwner {
    self.owner
  }
  pub(crate) fn refundable_owner(&self) -> Option<Address> {
    match self.owner {
      StorageOwner::Address(address) => Some(address),
      _ => None,
    }
  }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodeEntry {
  pub(crate) code: Arc<Vec<u8>>,
  pub(crate) owner: Option<Address>,
}

impl CodeEntry {
  pub fn new(code: Arc<Vec<u8>>, owner: Address) -> Self {
    Self {
      code,
      owner: Some(owner),
    }
  }
  pub fn with_unknown_owner(code: Arc<Vec<u8>>) -> Self {
    Self { code, owner: None }
  }
  pub fn code(&self) -> &Arc<Vec<u8>> {
    &self.code
  }
  pub fn owner(&self) -> Option<Address> {
    self.owner
  }
  pub fn code_size(&self) -> usize {
    self.code.len()
  }
  pub fn is_default(&self) -> bool {
    self.code.is_empty() && self.owner.unwrap_or_default().is_zero()
  }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WriteCacheItem {
  Read,
  Write(StorageEntry),
}
