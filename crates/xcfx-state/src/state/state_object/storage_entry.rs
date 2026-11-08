use super::{State, Substate};
use crate::database::StorageEntry;
use crate::{return_if, try_loaded};
use cfx_parameters::internal_contract_addresses::{
  SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS, SYSTEM_STORAGE_ADDRESS,
};
use cfx_statedb::Result as DbResult;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, BigEndianHash, Space, H256, U256};

impl State<'_> {
  // System Storage shares the cache and checkpoint mechanisms with
  // `OverlayAccount` storage entries. Similar to global statistic
  // variables, it represents global variables of the blockchain system,
  // operating without an owner during execution. As such, system storage
  // doesn't generate collateral, nor is it recorded in receipts.

  // While its access performance is slightly lower than global statistics due
  // to the cache and checkpoint mechanism, it benefits code maintainability.
  // New global variables are preferentially stored in system storage.
  pub fn get_system_storage(&self, key: &[u8]) -> DbResult<U256> {
    self.storage_at(&SYSTEM_STORAGE_ADDRESS.with_native_space(), key)
  }

  pub fn set_system_storage(&mut self, key: Vec<u8>, value: U256) -> DbResult<()> {
    // The system storage contract does not have owner, and thus does not
    // require actual storage owner and substate which records ownership
    // changes.
    self.set_storage(
      &SYSTEM_STORAGE_ADDRESS.with_native_space(),
      key,
      value,
      Address::zero(),
      &mut Substate::new(),
    )
  }

  pub fn set_eip2935_storage(&mut self, block_height: u64, block_hash: H256) -> DbResult<()> {
    use cfx_types::H160;
    use hex_literal::hex;

    pub const EIP2935_ADDRESS: AddressWithSpace = AddressWithSpace {
      address: H160(hex!("0000F90827F1C53a10cb7A02335B175320002935")),
      space: Space::Ethereum,
    };
    pub const HISTORY_SERVE_WINDOW: usize = 8191;

    if self.has_no_code(&EIP2935_ADDRESS)? {
      return Ok(());
    }

    let slot_index = U256::from(block_height % HISTORY_SERVE_WINDOW as u64);
    let key: H256 = BigEndianHash::from_uint(&slot_index);

    // The espace does not have owner.
    self.set_storage(
      &EIP2935_ADDRESS,
      key.0.to_vec(),
      block_hash.into_uint(),
      Address::zero(),
      &mut Substate::new(),
    )
  }

  #[inline]
  pub fn storage_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<U256> {
    let acc = try_loaded!(self.read_account_lock(address));
    acc.storage_at(&*self.db, key)
  }

  #[inline]
  pub fn origin_storage_at(
    &self,
    address: &AddressWithSpace,
    key: &[u8],
  ) -> DbResult<Option<U256>> {
    let acc = try_loaded!(self.read_account_lock(address));
    Ok(acc.origin_storage_at(key))
  }

  #[inline]
  pub fn transient_storage_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<U256> {
    let acc = try_loaded!(self.read_account_lock(address));
    Ok(acc.transient_storage_at(key))
  }

  #[inline]
  pub fn storage_entry_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<StorageEntry> {
    let acc = try_loaded!(self.read_account_lock(address));
    acc.storage_entry_at(&*self.db, key)
  }

  #[inline]
  pub fn set_storage(
    &mut self,
    address: &AddressWithSpace,
    key: Vec<u8>,
    value: U256,
    owner: Address,
    substate: &mut Substate,
  ) -> DbResult<()> {
    let old_value = self.storage_entry_at(address, &key)?;
    return_if!(old_value.value == value && !Self::force_reset_owner(address));

    self
      .write_account_lock(address)?
      .set_storage(key, value, old_value, owner, substate);

    Ok(())
  }

  #[inline]
  pub fn transient_set_storage(
    &mut self,
    address: &AddressWithSpace,
    key: Vec<u8>,
    value: U256,
  ) -> DbResult<()> {
    Ok(
      self
        .write_account_lock(address)?
        .transient_set_storage(key, value),
    )
  }

  pub fn is_fresh_storage(&self, address: &AddressWithSpace) -> DbResult<bool> {
    let acc = try_loaded!(self.read_account_lock(address));
    Ok(acc.fresh_storage())
  }

  // In most cases, the ownership does not change if the set storage operation
  // does not change the value. However, some implementations do not follow
  // this rule. So we must deal with these special cases for backward
  // compatible.
  #[inline]
  fn force_reset_owner(address: &AddressWithSpace) -> bool {
    address.space == Space::Native && address.address == SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS
  }
}
