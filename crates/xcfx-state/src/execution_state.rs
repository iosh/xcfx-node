use crate::State;
use cfx_bytes::Bytes;
use cfx_executor::{
  execution_state::ExecutionState, executive_observer::TracerTrait, state::CollateralCheckResult,
  substate::Substate,
};
use cfx_parameters::staking::code_collateral_units;
use cfx_statedb::Result as DbResult;
use cfx_types::{Address, AddressWithSpace, Space, H256, U256};
use cfx_vm_types::{ActionParams, Spec};
use primitives::{AccessListItem, StorageLayout};
use std::sync::Arc;

impl ExecutionState for State<'_> {
  fn exists(&self, address: &AddressWithSpace) -> DbResult<bool> {
    State::exists(self, address)
  }

  fn touch(&self, address: &AddressWithSpace) -> DbResult<()> {
    State::touch(self, address)
  }

  fn exists_and_not_null(&self, address: &AddressWithSpace) -> DbResult<bool> {
    State::exists_and_not_null(self, address)
  }

  fn balance(&self, address: &AddressWithSpace) -> DbResult<U256> {
    State::balance(self, address)
  }

  fn add_balance(&mut self, address: &AddressWithSpace, by: &U256) -> DbResult<()> {
    State::add_balance(self, address, by)
  }

  fn sub_balance(&mut self, address: &AddressWithSpace, by: &U256) -> DbResult<()> {
    State::sub_balance(self, address, by)
  }

  fn transfer_balance(
    &mut self,
    from: &AddressWithSpace,
    to: &AddressWithSpace,
    by: &U256,
  ) -> DbResult<()> {
    State::transfer_balance(self, from, to, by)
  }

  fn nonce(&self, address: &AddressWithSpace) -> DbResult<U256> {
    State::nonce(self, address)
  }

  fn inc_nonce(&mut self, address: &AddressWithSpace) -> DbResult<bool> {
    State::inc_nonce(self, address)
  }

  fn is_contract_with_code(&self, address: &AddressWithSpace) -> DbResult<bool> {
    State::is_contract_with_code(self, address)
  }

  fn is_eip684_empty(&self, address: &AddressWithSpace) -> DbResult<bool> {
    State::is_eip684_empty(self, address)
  }

  fn is_eip158_empty(&self, address: &AddressWithSpace) -> DbResult<bool> {
    State::is_eip158_empty(self, address)
  }

  fn code_hash(&self, address: &AddressWithSpace) -> DbResult<H256> {
    State::code_hash(self, address)
  }

  fn code_size(&self, address: &AddressWithSpace) -> DbResult<usize> {
    State::code_size(self, address)
  }

  fn code(&self, address: &AddressWithSpace) -> DbResult<Option<Arc<Vec<u8>>>> {
    State::code(self, address)
  }

  fn code_with_hash_on_call(
    &self,
    address: &AddressWithSpace,
  ) -> DbResult<(Option<Arc<Vec<u8>>>, H256)> {
    State::code_with_hash_on_call(self, address)
  }

  fn init_code(
    &mut self,
    address: &AddressWithSpace,
    code: Bytes,
    owner: Address,
    transaction_hash: H256,
  ) -> DbResult<()> {
    State::init_code(self, address, code, owner, transaction_hash)
  }

  fn created_at_transaction(
    &self,
    address: &AddressWithSpace,
    transaction_hash: H256,
  ) -> DbResult<bool> {
    State::created_at_transaction(self, address, transaction_hash)
  }

  fn set_authorization(&mut self, authority: &AddressWithSpace, address: &Address) -> DbResult<()> {
    State::set_authorization(self, authority, address)
  }

  fn admin(&self, address: &Address) -> DbResult<Address> {
    State::admin(self, address)
  }

  fn set_admin(&mut self, contract_address: &Address, admin: &Address) -> DbResult<()> {
    State::set_admin(self, contract_address, admin)
  }

  fn checkpoint(&mut self) -> usize {
    State::checkpoint(self)
  }

  fn discard_checkpoint(&mut self) {
    State::discard_checkpoint(self)
  }

  fn revert_to_checkpoint(&mut self) {
    State::revert_to_checkpoint(self)
  }

  fn no_checkpoint(&self) -> bool {
    State::no_checkpoint(self)
  }

  fn collateral_for_storage(&self, address: &Address) -> DbResult<U256> {
    State::collateral_for_storage(self, address)
  }

  fn token_collateral_for_storage(&self, address: &Address) -> DbResult<U256> {
    State::token_collateral_for_storage(self, address)
  }

  fn available_storage_points_for_collateral(&self, address: &Address) -> DbResult<U256> {
    State::available_storage_points_for_collateral(self, address)
  }

  fn check_storage_limit(
    &self,
    original_sender: &Address,
    storage_limit: &U256,
    dry_run: bool,
  ) -> DbResult<CollateralCheckResult> {
    State::check_storage_limit(self, original_sender, storage_limit, dry_run)
  }

  fn commit_cache(&mut self, retain_transient_storage: bool) {
    State::commit_cache(self, retain_transient_storage)
  }

  fn new_contract_with_admin(
    &mut self,
    contract: &AddressWithSpace,
    admin: &Address,
    balance: U256,
    storage_layout: Option<StorageLayout>,
    cip107: bool,
  ) -> DbResult<()> {
    State::new_contract_with_admin(self, contract, admin, balance, storage_layout, cip107)
  }

  fn remove_contract(&mut self, address: &AddressWithSpace) -> DbResult<()> {
    State::remove_contract(self, address)
  }

  fn sub_total_issued(&mut self, v: U256) {
    State::sub_total_issued(self, v)
  }

  fn add_total_pos_staking(&mut self, v: U256) {
    State::add_total_pos_staking(self, v)
  }

  fn add_total_evm_tokens(&mut self, v: U256) {
    State::add_total_evm_tokens(self, v)
  }

  fn sub_total_evm_tokens(&mut self, v: U256) {
    State::sub_total_evm_tokens(self, v)
  }

  fn total_pos_staking_tokens(&self) -> U256 {
    State::total_pos_staking_tokens(self)
  }

  fn inc_distributable_pos_interest(&mut self, current_block_number: u64) -> DbResult<()> {
    State::inc_distributable_pos_interest(self, current_block_number)
  }

  fn pos_locked_staking(&self, address: &Address) -> DbResult<U256> {
    State::pos_locked_staking(self, address)
  }

  fn bump_block_number_accumulate_interest(&mut self) {
    State::bump_block_number_accumulate_interest(self)
  }

  fn secondary_reward(&self) -> U256 {
    State::secondary_reward(self)
  }

  fn sponsor_for_gas(&self, address: &Address) -> DbResult<Option<Address>> {
    State::sponsor_for_gas(self, address)
  }

  fn set_sponsor_for_gas(
    &self,
    address: &Address,
    sponsor: &Address,
    sponsor_balance: &U256,
    upper_bound: &U256,
  ) -> DbResult<()> {
    State::set_sponsor_for_gas(self, address, sponsor, sponsor_balance, upper_bound)
  }

  fn sponsor_balance_for_gas(&self, address: &Address) -> DbResult<U256> {
    State::sponsor_balance_for_gas(self, address)
  }

  fn add_sponsor_balance_for_gas(&mut self, address: &Address, by: &U256) -> DbResult<()> {
    State::add_sponsor_balance_for_gas(self, address, by)
  }

  fn sub_sponsor_balance_for_gas(&mut self, address: &Address, by: &U256) -> DbResult<()> {
    State::sub_sponsor_balance_for_gas(self, address, by)
  }

  fn sponsor_gas_bound(&self, address: &Address) -> DbResult<U256> {
    State::sponsor_gas_bound(self, address)
  }

  fn sponsor_for_collateral(&self, address: &Address) -> DbResult<Option<Address>> {
    State::sponsor_for_collateral(self, address)
  }

  fn set_sponsor_for_collateral(
    &mut self,
    address: &Address,
    sponsor: &Address,
    sponsor_balance: &U256,
    is_cip107: bool,
  ) -> DbResult<U256> {
    State::set_sponsor_for_collateral(self, address, sponsor, sponsor_balance, is_cip107)
  }

  fn sponsor_balance_for_collateral(&self, address: &Address) -> DbResult<U256> {
    State::sponsor_balance_for_collateral(self, address)
  }

  fn sub_sponsor_balance_for_collateral(&mut self, address: &Address, by: &U256) -> DbResult<()> {
    State::sub_sponsor_balance_for_collateral(self, address, by)
  }

  fn check_contract_whitelist(&self, contract_address: &Address, user: &Address) -> DbResult<bool> {
    State::check_contract_whitelist(self, contract_address, user)
  }

  fn add_to_contract_whitelist(
    &mut self,
    contract_address: Address,
    storage_owner: Address,
    user: Address,
    substate: &mut Substate,
  ) -> DbResult<()> {
    State::add_to_contract_whitelist(self, contract_address, storage_owner, user, substate)
  }

  fn remove_from_contract_whitelist(
    &mut self,
    contract_address: Address,
    storage_owner: Address,
    user: Address,
    substate: &mut Substate,
  ) -> DbResult<()> {
    State::remove_from_contract_whitelist(self, contract_address, storage_owner, user, substate)
  }

  fn staking_balance(&self, address: &Address) -> DbResult<U256> {
    State::staking_balance(self, address)
  }

  fn withdrawable_staking_balance(
    &self,
    address: &Address,
    current_block_number: u64,
  ) -> DbResult<U256> {
    State::withdrawable_staking_balance(self, address, current_block_number)
  }

  fn locked_staking_balance_at_block_number(
    &self,
    address: &Address,
    block_number: u64,
  ) -> DbResult<U256> {
    State::locked_staking_balance_at_block_number(self, address, block_number)
  }

  fn vote_stake_list_length(&self, address: &Address) -> DbResult<usize> {
    State::vote_stake_list_length(self, address)
  }

  fn vote_lock(
    &mut self,
    address: &Address,
    amount: &U256,
    unlock_block_number: u64,
  ) -> DbResult<()> {
    State::vote_lock(self, address, amount, unlock_block_number)
  }

  fn remove_expired_vote_stake_info(
    &mut self,
    address: &Address,
    current_block_number: u64,
  ) -> DbResult<()> {
    State::remove_expired_vote_stake_info(self, address, current_block_number)
  }

  fn deposit_list_length(&self, address: &Address) -> DbResult<usize> {
    State::deposit_list_length(self, address)
  }

  fn deposit(
    &mut self,
    address: &Address,
    amount: &U256,
    current_block_number: u64,
    cip_97: bool,
  ) -> DbResult<()> {
    State::deposit(self, address, amount, current_block_number, cip_97)
  }

  fn withdraw(&mut self, address: &Address, amount: &U256, cip_97: bool) -> DbResult<U256> {
    State::withdraw(self, address, amount, cip_97)
  }

  fn get_system_storage(&self, key: &[u8]) -> DbResult<U256> {
    State::get_system_storage(self, key)
  }

  fn set_system_storage(&mut self, key: Vec<u8>, value: U256) -> DbResult<()> {
    State::set_system_storage(self, key, value)
  }

  fn set_eip2935_storage(&mut self, block_height: u64, block_hash: H256) -> DbResult<()> {
    State::set_eip2935_storage(self, block_height, block_hash)
  }

  fn storage_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<U256> {
    State::storage_at(self, address, key)
  }

  fn origin_storage_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<Option<U256>> {
    State::origin_storage_at(self, address, key)
  }

  fn transient_storage_at(&self, address: &AddressWithSpace, key: &[u8]) -> DbResult<U256> {
    State::transient_storage_at(self, address, key)
  }

  fn set_storage(
    &mut self,
    address: &AddressWithSpace,
    key: Vec<u8>,
    value: U256,
    owner: Address,
    substate: &mut Substate,
  ) -> DbResult<()> {
    State::set_storage(self, address, key, value, owner, substate)
  }

  fn transient_set_storage(
    &mut self,
    address: &AddressWithSpace,
    key: Vec<u8>,
    value: U256,
  ) -> DbResult<()> {
    State::transient_set_storage(self, address, key, value)
  }

  fn is_warm_account(&self, address: &AddressWithSpace) -> bool {
    State::is_warm_account(self, address)
  }

  fn is_warm_storage_entry(&self, address: &AddressWithSpace, key: &H256) -> DbResult<bool> {
    State::is_warm_storage_entry(self, address, key)
  }

  fn set_tx_access_list(&mut self, space: Space, access_list: &[AccessListItem], cip176: bool) {
    State::set_tx_access_list(self, space, access_list, cip176)
  }

  fn touch_tx_addresses(&self, params: &ActionParams) -> DbResult<()> {
    State::touch_tx_addresses(self, params)
  }
  fn assert_transaction_ready(&mut self) {
    assert!(
      self.cache.get_mut().is_empty(),
      "transaction state must be finalized before execution"
    );
  }
  fn settle_collateral_for_all(
    &mut self,
    substate: &Substate,
    tracer: &mut dyn TracerTrait,
    spec: &Spec,
    dry_run: bool,
  ) -> DbResult<CollateralCheckResult> {
    crate::state::settle_collateral_for_all(self, substate, tracer, spec, dry_run)
  }
  fn record_contract_collateral_release(
    &mut self,
    address: &AddressWithSpace,
    substate: &mut Substate,
    cip131: bool,
  ) -> DbResult<()> {
    let code_size = self.code_size(address)?;
    if code_size > 0 {
      if let Some(owner) = self.code_owner(address)? {
        substate.record_storage_release(&owner, code_collateral_units(code_size));
      }
    }
    self.record_storage_and_whitelist_entries_release(&address.address, substate, cip131)?;
    assert!(self.is_fresh_storage(address)?);
    Ok(())
  }
  fn initialize_cip107_globals(&mut self) -> DbResult<()> {
    crate::state::initialize_cip107(self)
  }
  fn initialize_cip137_globals(&mut self) {
    crate::state::initialize_cip137(self)
  }
  fn initialize_or_update_dao_voted_params(&mut self, cip105: bool) -> DbResult<()> {
    crate::state::initialize_or_update_dao_voted_params(self, cip105)
  }
}
