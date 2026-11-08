use super::{OverlayAccount, State};
use crate::account::Account;
use cfx_statedb::Result as DbResult;
use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, Space, H256, U256};
use primitives::StorageLayout;

impl State<'_> {
  pub fn new_contract_with_admin(
    &mut self,
    contract: &AddressWithSpace,
    admin: &Address,
    balance: U256,
    storage_layout: Option<StorageLayout>,
    cip107: bool,
  ) -> DbResult<()> {
    assert!(contract.space == Space::Native || admin.is_zero());
    // Check if the new contract is deployed on a killed contract in the
    // same block.
    let pending_db_clear = self
      .read_account_lock(contract)?
      .map_or(false, |overlay| overlay.pending_db_clear());
    let account = OverlayAccount::new_contract_with_admin(
      contract,
      balance,
      admin,
      pending_db_clear,
      storage_layout,
      cip107,
    );
    self.insert_to_cache(account);
    Ok(())
  }

  /// Kill a contract
  pub fn remove_contract(&mut self, address: &AddressWithSpace) -> DbResult<()> {
    self.insert_to_cache(OverlayAccount::new_removed(address));

    Ok(())
  }

  /// A special implementation to achieve the backward compatible for the
  /// genesis (incorrect) behaviour.
  pub fn genesis_special_remove_account(&mut self, address: &Address) -> DbResult<()> {
    let address = address.with_native_space();
    let mut account = Account::new_empty(&address);
    account.code_hash = H256::default();
    *self.write_account_or_new_lock(&address)? = OverlayAccount::from_loaded(&address, account);
    Ok(())
  }
}
