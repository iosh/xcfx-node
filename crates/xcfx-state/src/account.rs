//! Account data, including information not exposed by public RPC.
use cfx_types::{Address, AddressWithSpace, H256, U256};
use keccak_hash::KECCAK_EMPTY;
use primitives::account::StoragePoints;

/// Storage-point amounts and whether CIP-107 initialization is known.
/// Unknown is reserved for inherited accounts whose public point amounts are zero.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum StoragePointsState {
  #[default]
  Uninitialized,
  Initialized(StoragePoints),
  Unknown,
}

impl StoragePointsState {
  pub fn initialization(&self) -> Option<bool> {
    match self {
      Self::Uninitialized => Some(false),
      Self::Initialized(_) => Some(true),
      Self::Unknown => None,
    }
  }

  pub fn unused(&self) -> U256 {
    match self {
      Self::Initialized(points) => points.unused,
      _ => U256::zero(),
    }
  }

  pub fn used(&self) -> U256 {
    match self {
      Self::Initialized(points) => points.used,
      _ => U256::zero(),
    }
  }

  pub(crate) fn initialized_mut(&mut self) -> &mut StoragePoints {
    match self {
      Self::Initialized(points) => points,
      _ => panic!("nonzero storage points require an initialized account"),
    }
  }

  pub(crate) fn resolve(&mut self, initialized: bool) {
    assert!(
      matches!(self, Self::Unknown),
      "only inherited unknown initialization may be resolved"
    );
    *self = if initialized {
      Self::Initialized(StoragePoints::default())
    } else {
      Self::Uninitialized
    };
  }
}

/// Sponsorship data stored once with the account.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SponsorInfo {
  pub sponsor_for_gas: Address,
  pub sponsor_for_collateral: Address,
  pub sponsor_gas_bound: U256,
  pub sponsor_balance_for_gas: U256,
  pub sponsor_balance_for_collateral: U256,
  pub storage_points: StoragePointsState,
}

impl SponsorInfo {
  pub fn unused_storage_points(&self) -> U256 {
    self.storage_points.unused()
  }

  pub(crate) fn default_status(&self) -> Option<bool> {
    if !self.sponsor_for_gas.is_zero()
      || !self.sponsor_for_collateral.is_zero()
      || !self.sponsor_gas_bound.is_zero()
      || !self.sponsor_balance_for_gas.is_zero()
      || !self.sponsor_balance_for_collateral.is_zero()
    {
      return Some(false);
    }
    self
      .storage_points
      .initialization()
      .map(|initialized| !initialized)
  }

  pub(crate) fn as_conflux(&self) -> primitives::SponsorInfo {
    let storage_points = match &self.storage_points {
      StoragePointsState::Uninitialized => None,
      StoragePointsState::Initialized(points) => Some(points.clone()),
      StoragePointsState::Unknown => panic!("MPT account encoding requires known initialization"),
    };
    primitives::SponsorInfo {
      sponsor_for_gas: self.sponsor_for_gas,
      sponsor_for_collateral: self.sponsor_for_collateral,
      sponsor_gas_bound: self.sponsor_gas_bound,
      sponsor_balance_for_gas: self.sponsor_balance_for_gas,
      sponsor_balance_for_collateral: self.sponsor_balance_for_collateral,
      storage_points,
    }
  }
}

impl From<primitives::SponsorInfo> for SponsorInfo {
  fn from(value: primitives::SponsorInfo) -> Self {
    Self {
      sponsor_for_gas: value.sponsor_for_gas,
      sponsor_for_collateral: value.sponsor_for_collateral,
      sponsor_gas_bound: value.sponsor_gas_bound,
      sponsor_balance_for_gas: value.sponsor_balance_for_gas,
      sponsor_balance_for_collateral: value.sponsor_balance_for_collateral,
      storage_points: value
        .storage_points
        .map(StoragePointsState::Initialized)
        .unwrap_or_default(),
    }
  }
}

/// One account record shared by state caches and backing versions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Account {
  address: AddressWithSpace,
  pub balance: U256,
  pub nonce: U256,
  pub code_hash: H256,
  pub staking_balance: U256,
  pub collateral_for_storage: U256,
  pub accumulated_interest_return: U256,
  pub admin: Address,
  pub sponsor_info: SponsorInfo,
}

impl Account {
  pub fn new_empty(address: &AddressWithSpace) -> Self {
    Self {
      address: *address,
      balance: U256::zero(),
      nonce: U256::zero(),
      code_hash: KECCAK_EMPTY,
      staking_balance: U256::zero(),
      collateral_for_storage: U256::zero(),
      accumulated_interest_return: U256::zero(),
      admin: Address::zero(),
      sponsor_info: SponsorInfo::default(),
    }
  }

  pub fn address(&self) -> &AddressWithSpace {
    &self.address
  }

  /// None means that deleting an otherwise-default account requires its
  /// inherited initialization flag; non-default fields are decisive on their own.
  pub fn default_status(&self) -> Option<bool> {
    if !self.balance.is_zero()
      || !self.nonce.is_zero()
      || self.code_hash != KECCAK_EMPTY
      || !self.staking_balance.is_zero()
      || !self.collateral_for_storage.is_zero()
      || !self.accumulated_interest_return.is_zero()
      || !self.admin.is_zero()
    {
      return Some(false);
    }
    self.sponsor_info.default_status()
  }

  /// State writeback resolves initialization before an ambiguous deletion.
  pub fn is_default(&self) -> bool {
    self
      .default_status()
      .expect("account deletion requires resolved initialization")
  }

  pub(crate) fn from_conflux(address: &AddressWithSpace, value: primitives::Account) -> Self {
    Self {
      address: *address,
      balance: value.balance,
      nonce: value.nonce,
      code_hash: value.code_hash,
      staking_balance: value.staking_balance,
      collateral_for_storage: value.collateral_for_storage,
      accumulated_interest_return: value.accumulated_interest_return,
      admin: value.admin,
      sponsor_info: value.sponsor_info.into(),
    }
  }

  pub(crate) fn as_conflux(&self) -> primitives::Account {
    let mut account = primitives::Account::new_empty(&self.address);
    account.balance = self.balance;
    account.nonce = self.nonce;
    account.code_hash = self.code_hash;
    account.staking_balance = self.staking_balance;
    account.collateral_for_storage = self.collateral_for_storage;
    account.accumulated_interest_return = self.accumulated_interest_return;
    account.admin = self.admin;
    account.sponsor_info = self.sponsor_info.as_conflux();
    account
  }
}
