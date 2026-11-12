//! Core state fields read inside the validated fixed remote prefix.

use std::sync::Arc;

use cfx_rpc_cfx_types::{EpochNumber, RpcAddress};
use cfx_types::{Address, BigEndianHash, H256};
use primitives::block::BlockHeight;

use crate::native_state;

use super::{CoreStateQuery, ForkReadError, ForkRpc, HistoryResult};

impl ForkRpc {
  async fn core_state_pivot_hash(&self, epoch_height: BlockHeight) -> Result<H256, ForkReadError> {
    if epoch_height == self.base.epoch_height {
      Ok(self.base.pivot_hash)
    } else {
      Ok(self.load_historical_pivot(epoch_height).await?.hash)
    }
  }

  fn core_response_address(&self, address: RpcAddress) -> Result<Address, ForkReadError> {
    if address.network != self.core_address(address.hex_address).network {
      return Err(ForkReadError::Inconsistent("Core response address network"));
    }

    Ok(address.hex_address)
  }

  pub(super) async fn read_core_state(
    &self,
    epoch_height: BlockHeight,
    query: CoreStateQuery,
  ) -> Result<HistoryResult, ForkReadError> {
    let epoch = EpochNumber::Num(epoch_height.into());

    Ok(match query {
      CoreStateQuery::Balance(address) => {
        let pivot_hash = self.core_state_pivot_hash(epoch_height).await?;
        HistoryResult::CoreQuantity(
          self
            .rpc
            .cfx_get_balance(self.core_address(address), pivot_hash)
            .await?,
        )
      }
      CoreStateQuery::Nonce(address) => {
        let pivot_hash = self.core_state_pivot_hash(epoch_height).await?;
        HistoryResult::CoreQuantity(
          self
            .rpc
            .cfx_get_next_nonce(self.core_address(address), pivot_hash)
            .await?,
        )
      }
      CoreStateQuery::Code(address) => {
        let pivot_hash = self.core_state_pivot_hash(epoch_height).await?;
        HistoryResult::CoreCode(Arc::new(
          self
            .rpc
            .cfx_get_code(self.core_address(address), pivot_hash)
            .await?
            .into_vec(),
        ))
      }
      CoreStateQuery::Storage { address, position } => {
        let pivot_hash = self.core_state_pivot_hash(epoch_height).await?;
        HistoryResult::CoreStorage(
          self
            .rpc
            .cfx_get_storage_at(self.core_address(address), position.into_uint(), pivot_hash)
            .await?,
        )
      }
      CoreStateQuery::Admin(address) => {
        let admin = self
          .rpc
          .cfx_get_admin(self.core_address(address), epoch)
          .await?;
        HistoryResult::CoreAdmin(
          admin
            .map(|address| self.core_response_address(address))
            .transpose()?,
        )
      }

      CoreStateQuery::SponsorInfo(address) => {
        let sponsor = self
          .rpc
          .cfx_get_sponsor_info(self.core_address(address), epoch)
          .await?;

        HistoryResult::CoreSponsorInfo(Arc::new(native_state::SponsorInfo {
          sponsor_for_gas: self.core_response_address(sponsor.sponsor_for_gas)?,
          sponsor_for_collateral: self.core_response_address(sponsor.sponsor_for_collateral)?,
          sponsor_gas_bound: sponsor.sponsor_gas_bound,
          sponsor_balance_for_gas: sponsor.sponsor_balance_for_gas,
          sponsor_balance_for_collateral: sponsor.sponsor_balance_for_collateral,
          available_storage_points: sponsor.available_storage_points,
          used_storage_points: sponsor.used_storage_points,
        }))
      }

      CoreStateQuery::StakingBalance(address) => HistoryResult::CoreQuantity(
        self
          .rpc
          .cfx_get_staking_balance(self.core_address(address), epoch)
          .await?,
      ),

      CoreStateQuery::StorageCollateral(address) => HistoryResult::CoreQuantity(
        self
          .rpc
          .cfx_get_collateral_for_storage(self.core_address(address), epoch)
          .await?,
      ),

      CoreStateQuery::DepositList(address) => HistoryResult::CoreDepositList(Arc::new(
        self
          .rpc
          .cfx_get_deposit_list(self.core_address(address), epoch)
          .await?,
      )),

      CoreStateQuery::VoteList(address) => HistoryResult::CoreVoteList(Arc::new(
        self
          .rpc
          .cfx_get_vote_list(self.core_address(address), epoch)
          .await?,
      )),

      CoreStateQuery::Account(address) => {
        let account = self
          .rpc
          .cfx_get_account(self.core_address(address), epoch)
          .await?;

        if self.core_response_address(account.address)? != address {
          return Err(ForkReadError::Inconsistent("Core account address"));
        }

        HistoryResult::CoreAccount(Arc::new(native_state::AccountInfo {
          address,
          balance: account.balance,
          nonce: account.nonce,
          code_hash: account.code_hash,
          staking_balance: account.staking_balance,
          collateral_for_storage: account.collateral_for_storage,
          accumulated_interest_return: account.accumulated_interest_return,
          admin: self.core_response_address(account.admin)?,
        }))
      }

      CoreStateQuery::InterestRate => {
        HistoryResult::CoreQuantity(self.rpc.cfx_get_interest_rate(epoch).await?)
      }
      CoreStateQuery::AccumulateInterestRate => {
        HistoryResult::CoreQuantity(self.rpc.cfx_get_accumulate_interest_rate(epoch).await?)
      }
      CoreStateQuery::FeeBurnt => {
        HistoryResult::CoreQuantity(self.rpc.cfx_get_fee_burnt(epoch).await?)
      }

      CoreStateQuery::PosEconomics => {
        let economics = self.rpc.cfx_get_pos_economics(epoch).await?;

        HistoryResult::CorePosEconomics(Arc::new(native_state::PosEconomics {
          total_pos_staking_tokens: economics.total_pos_staking_tokens,
          distributable_pos_interest: economics.distributable_pos_interest,
          last_distribute_block: economics.last_distribute_block.as_u64(),
        }))
      }

      CoreStateQuery::SupplyInfo => {
        let supply = self.rpc.cfx_get_supply_info(epoch).await?;

        HistoryResult::CoreSupplyInfo(Arc::new(native_state::TokenSupplyInfo {
          total_circulating: supply.total_circulating,
          total_issued: supply.total_issued,
          total_staking: supply.total_staking,
          total_collateral: supply.total_collateral,
          total_espace_tokens: supply.total_espace_tokens,
        }))
      }

      CoreStateQuery::CollateralInfo => {
        let collateral = self.rpc.cfx_get_collateral_info(epoch).await?;

        HistoryResult::CoreCollateralInfo(Arc::new(native_state::StorageCollateralInfo {
          total_storage_tokens: collateral.total_storage_tokens,
          converted_storage_points: collateral.converted_storage_points,
          used_storage_points: collateral.used_storage_points,
        }))
      }

      CoreStateQuery::VoteParams => {
        let params = self.rpc.cfx_get_params_from_vote(epoch).await?;

        HistoryResult::CoreVoteParams(Arc::new(native_state::VoteParams {
          pow_base_reward: params.pow_base_reward,
          interest_rate: params.interest_rate,
          storage_point_prop: params.storage_point_prop,
          base_fee_share_prop: params.base_fee_share_prop,
        }))
      }

      CoreStateQuery::CheckBalance(native_state::BalanceCheckRequest {
        account,
        contract,
        gas_limit,
        gas_price,
        storage_limit,
      }) => {
        let check = self
          .rpc
          .cfx_check_balance_against_transaction(
            self.core_address(account),
            self.core_address(contract),
            gas_limit,
            gas_price,
            storage_limit.into(),
            epoch,
          )
          .await?;

        HistoryResult::CoreBalanceCheck(native_state::BalanceCheckResult {
          will_pay_tx_fee: check.will_pay_tx_fee,
          will_pay_collateral: check.will_pay_collateral,
          is_balance_enough: check.is_balance_enough,
        })
      }
    })
  }
}
