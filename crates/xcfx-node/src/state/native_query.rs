//! Native public-field reads over one immutable state version.

use cfx_executor::internal_contract::storage_point_prop;
use cfx_parameters::{
  genesis::{genesis_contract_address_four_year, genesis_contract_address_two_year},
  internal_contract_addresses::{
    SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS, SYSTEM_STORAGE_ADDRESS,
  },
  staking::{BLOCKS_PER_YEAR, DRIPS_PER_STORAGE_COLLATERAL_UNIT},
};
use cfx_statedb::global_params::*;
use cfx_types::{Address, AddressSpaceUtil, U256, U512};
use primitives::{DepositInfo, StorageKey, VoteStakeInfo};
use xcfx_state::{
  account::{Account, SponsorInfo as AccountSponsorInfo},
  database::{Database, MptDatabase},
};

use crate::{
  fork::{ForkClient, ForkReadError, StateValue},
  native_state::{
    AccountInfo, BalanceCheckRequest, BalanceCheckResult, PosEconomics, SponsorInfo,
    StorageCollateralInfo, TokenSupplyInfo, VoteParams,
  },
};

use super::{
  StateReadError, fork_state::ForkStateVersion, mpt_state::MptStateCandidate,
  state_version::StateVersion,
};

enum Backend<'a> {
  Mpt(Box<MptDatabase<MptStateCandidate>>),
  Fork(&'a ForkStateVersion),
}

/// Read-only access to the selected version; every result owns its public fields.
pub(crate) struct NativeStateReader<'a> {
  backend: Backend<'a>,
}

impl StateVersion {
  pub(crate) fn native_reader(&self) -> NativeStateReader<'_> {
    let backend = match self {
      Self::Mpt(version) => Backend::Mpt(Box::new(version.open_database())),
      Self::Fork(version) => Backend::Fork(version),
    };

    NativeStateReader { backend }
  }
}

impl NativeStateReader<'_> {
  fn inherited_account_client(&self, address: Address) -> Option<&ForkClient> {
    match &self.backend {
      Backend::Fork(version) if version.inherits_native_account(address) => {
        Some(version.base_client())
      }
      _ => None,
    }
  }

  fn account_record(&self, address: Address) -> Result<Option<Account>, StateReadError> {
    match &self.backend {
      Backend::Mpt(database) => Ok(database.get_account(&address.with_native_space())?),

      Backend::Fork(version) => Ok(
        version
          .query_value(StorageKey::new_account_key(&address).with_native_space())?
          .map(|record| record.account().clone()),
      ),
    }
  }

  fn balance(&self, address: Address) -> Result<U256, StateReadError> {
    match &self.backend {
      Backend::Mpt(database) => Ok(database.get_account_balance(&address.with_native_space())?),
      Backend::Fork(version) => version.native_balance(address),
    }
  }

  fn global_parameter<K: GlobalParamKey>(&self) -> Result<U256, StateReadError> {
    let value = match &self.backend {
      Backend::Mpt(database) => database.get_global_param(K::STORAGE_KEY)?,
      Backend::Fork(version) => version
        .query_value(K::STORAGE_KEY)?
        .map(|record| record.storage().value()),
    };

    Ok(value.unwrap_or_default())
  }

  fn storage(&self, address: Address, key: &[u8]) -> Result<U256, StateReadError> {
    match &self.backend {
      Backend::Mpt(database) => Ok(
        database
          .get_storage_entry(&address.with_native_space(), key)?
          .map_or(U256::zero(), |entry| entry.value()),
      ),

      Backend::Fork(version) => {
        let key = StorageKey::new_storage_key(&address, key).with_native_space();

        Ok(match version.query_value(key)? {
          None | Some(StateValue::HiddenWhitelistEntry) => U256::zero(),
          Some(record) => record.storage().value(),
        })
      }
    }
  }

  pub(crate) fn admin(&self, address: Address) -> Result<Option<Address>, StateReadError> {
    if let Some(client) = self.inherited_account_client(address) {
      return Ok(client.core_admin_at(address, client.base().epoch_height)?);
    }

    Ok(self.account_record(address)?.map(|account| account.admin))
  }

  pub(crate) fn sponsor_info(&self, address: Address) -> Result<SponsorInfo, StateReadError> {
    if let Some(client) = self.inherited_account_client(address) {
      return Ok(client.core_sponsor_info_at(address, client.base().epoch_height)?);
    }

    let sponsor = self
      .account_record(address)?
      .map(|account| account.sponsor_info)
      .unwrap_or_default();
    let drips_per_storage_unit = *DRIPS_PER_STORAGE_COLLATERAL_UNIT;

    Ok(SponsorInfo {
      sponsor_for_gas: sponsor.sponsor_for_gas,
      sponsor_for_collateral: sponsor.sponsor_for_collateral,
      sponsor_gas_bound: sponsor.sponsor_gas_bound,
      sponsor_balance_for_gas: sponsor.sponsor_balance_for_gas,
      sponsor_balance_for_collateral: sponsor.sponsor_balance_for_collateral,
      available_storage_points: sponsor.storage_points.unused() / drips_per_storage_unit,
      used_storage_points: sponsor.storage_points.used() / drips_per_storage_unit,
    })
  }

  pub(crate) fn staking_balance(&self, address: Address) -> Result<U256, StateReadError> {
    if let Some(client) = self.inherited_account_client(address) {
      return Ok(client.core_staking_balance_at(address, client.base().epoch_height)?);
    }

    Ok(
      self
        .account_record(address)?
        .map_or(U256::zero(), |account| account.staking_balance),
    )
  }

  pub(crate) fn collateral_for_storage(&self, address: Address) -> Result<U256, StateReadError> {
    if let Some(client) = self.inherited_account_client(address) {
      return Ok(client.core_collateral_for_storage_at(address, client.base().epoch_height)?);
    }

    Ok(
      self
        .account_record(address)?
        .map_or(U256::zero(), |account| account.collateral_for_storage),
    )
  }

  pub(crate) fn account(&self, address: Address) -> Result<AccountInfo, StateReadError> {
    if let Some(client) = self.inherited_account_client(address) {
      return Ok(client.core_account_at(address, client.base().epoch_height)?);
    }

    let account = self
      .account_record(address)?
      .unwrap_or_else(|| Account::new_empty(&address.with_native_space()));

    // Fork import splits one bounded public total into token collateral and
    // used points. Local transitions preserve the same account invariant.
    let collateral_for_storage = account
      .collateral_for_storage
      .checked_add(account.sponsor_info.storage_points.used())
      .expect("account collateral and used storage points must fit their public total");

    Ok(AccountInfo {
      address,
      balance: account.balance,
      nonce: account.nonce,
      code_hash: account.code_hash,
      staking_balance: account.staking_balance,
      collateral_for_storage,
      accumulated_interest_return: account.accumulated_interest_return,
      admin: account.admin,
    })
  }

  pub(crate) fn deposit_list(&self, address: Address) -> Result<Vec<DepositInfo>, StateReadError> {
    let deposits = match &self.backend {
      Backend::Mpt(database) => database.get_deposit_list(&address.with_native_space())?,
      Backend::Fork(version) => version
        .query_value(StorageKey::new_deposit_list_key(&address).with_native_space())?
        .map(|record| record.deposit_list().clone()),
    };

    Ok(deposits.unwrap_or_default().0)
  }

  pub(crate) fn vote_list(&self, address: Address) -> Result<Vec<VoteStakeInfo>, StateReadError> {
    let votes = match &self.backend {
      Backend::Mpt(database) => database.get_vote_list(&address.with_native_space())?,
      Backend::Fork(version) => version
        .query_value(StorageKey::new_vote_list_key(&address).with_native_space())?
        .map(|record| record.vote_list().clone()),
    };

    Ok(votes.unwrap_or_default().0)
  }

  pub(crate) fn interest_rate(&self) -> Result<U256, StateReadError> {
    self.global_parameter::<InterestRate>()
  }

  pub(crate) fn accumulated_interest_rate(&self) -> Result<U256, StateReadError> {
    self.global_parameter::<AccumulateInterestRate>()
  }

  pub(crate) fn fee_burnt(&self) -> Result<U256, StateReadError> {
    self.global_parameter::<TotalBurnt1559>()
  }

  pub(crate) fn pos_economics(&self) -> Result<PosEconomics, StateReadError> {
    let last_distribute_block = u64::try_from(self.global_parameter::<LastDistributeBlock>()?)
      .expect("the imported or locally maintained PoS distribution block must fit u64");

    Ok(PosEconomics {
      total_pos_staking_tokens: self.global_parameter::<TotalPosStaking>()?,
      distributable_pos_interest: self.global_parameter::<DistributablePoSInterest>()?,
      last_distribute_block,
    })
  }

  pub(crate) fn supply_info(&self) -> Result<TokenSupplyInfo, StateReadError> {
    let total_issued = self.global_parameter::<TotalIssued>()?;
    let two_year_locked = self.balance(genesis_contract_address_two_year().address)?;
    let four_year_locked = self.balance(genesis_contract_address_four_year().address)?;

    let total_circulating = total_issued
      .checked_sub(two_year_locked)
      .and_then(|remaining| remaining.checked_sub(four_year_locked));

    let total_circulating = match total_circulating {
      Some(supply) => supply,
      // Fork issuance and lazily loaded locked balances come from separate
      // remote responses; their consistency is not established at import.
      None if matches!(&self.backend, Backend::Fork(_)) => {
        return Err(ForkReadError::Inconsistent("locked supply exceeds total issuance").into());
      }
      None => panic!("local total issuance must cover both locked genesis balances"),
    };

    Ok(TokenSupplyInfo {
      total_circulating,
      total_issued,
      total_staking: self.global_parameter::<TotalStaking>()?,
      total_collateral: self.global_parameter::<TotalStorage>()?,
      total_espace_tokens: self.global_parameter::<TotalEvmToken>()?,
    })
  }

  pub(crate) fn collateral_info(&self) -> Result<StorageCollateralInfo, StateReadError> {
    let drips_per_storage_unit = *DRIPS_PER_STORAGE_COLLATERAL_UNIT;

    Ok(StorageCollateralInfo {
      total_storage_tokens: self.global_parameter::<TotalStorage>()?,
      converted_storage_points: self.global_parameter::<ConvertedStoragePoints>()?
        / drips_per_storage_unit,
      used_storage_points: self.global_parameter::<UsedStoragePoints>()? / drips_per_storage_unit,
    })
  }

  pub(crate) fn vote_params(&self) -> Result<VoteParams, StateReadError> {
    Ok(VoteParams {
      pow_base_reward: self.global_parameter::<PowBaseReward>()?,
      interest_rate: self.global_parameter::<InterestRate>()? / U256::from(BLOCKS_PER_YEAR),
      storage_point_prop: self.storage(SYSTEM_STORAGE_ADDRESS, &storage_point_prop())?,
      base_fee_share_prop: self.global_parameter::<BaseFeeProp>()?,
    })
  }

  fn is_whitelisted(&self, contract: Address, account: Address) -> Result<bool, StateReadError> {
    // A whitelist key concatenates the contract and user addresses; a zero
    // user grants access to every account.
    let mut key = [0; 40];
    key[..20].copy_from_slice(contract.as_bytes());

    if !self
      .storage(SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS, &key)?
      .is_zero()
    {
      return Ok(true);
    }

    key[20..].copy_from_slice(account.as_bytes());
    Ok(
      !self
        .storage(SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS, &key)?
        .is_zero(),
    )
  }

  pub(crate) fn check_balance(
    &self,
    request: BalanceCheckRequest,
  ) -> Result<BalanceCheckResult, StateReadError> {
    let balance = self.balance(request.account)?;
    let sponsor = self
      .account_record(request.contract)?
      .map(|account| account.sponsor_info)
      .unwrap_or_default();
    let is_whitelisted = self.is_whitelisted(request.contract, request.account)?;

    Ok(check_transaction_balance(
      request,
      balance,
      &sponsor,
      is_whitelisted,
    ))
  }
}

fn check_transaction_balance(
  request: BalanceCheckRequest,
  balance: U256,
  sponsor: &AccountSponsorInfo,
  is_whitelisted: bool,
) -> BalanceCheckResult {
  let gas_cost = request.gas_limit.full_mul(request.gas_price);
  let storage_cost = U256::from(request.storage_limit).full_mul(*DRIPS_PER_STORAGE_COLLATERAL_UNIT);

  let will_pay_tx_fee = !is_whitelisted
    || sponsor.sponsor_for_gas.is_zero()
    || gas_cost > U512::from(sponsor.sponsor_gas_bound)
    || gas_cost > U512::from(sponsor.sponsor_balance_for_gas);

  let collateral_balance = U512::from(sponsor.sponsor_balance_for_collateral)
    + U512::from(sponsor.storage_points.unused());
  let will_pay_collateral = !is_whitelisted
    || sponsor.sponsor_for_collateral.is_zero()
    || storage_cost > collateral_balance;

  let required_balance = match (will_pay_tx_fee, will_pay_collateral) {
    (false, false) => U512::zero(),
    (true, false) => gas_cost,
    (false, true) => storage_cost,
    (true, true) => gas_cost + storage_cost,
  };

  BalanceCheckResult {
    will_pay_tx_fee,
    will_pay_collateral,
    is_balance_enough: U512::from(balance) >= required_balance,
  }
}
