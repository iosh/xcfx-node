pub(super) mod convert;
mod error;
mod fees;
mod params;
mod pool;
mod query;
mod state;

use cfx_rpc_cfx_types::{
  BlockHashOrEpochNumber, Bytes, EpochNumber, EstimateGasAndCollateralResponse, RpcAddress,
};
use cfx_types::{BigEndianHash, H128, H256, H520, Space, U64, U256};
use serde::Deserialize;
use serde_json::json;

use crate::logs::PendingTransactionFilter;
use crate::runtime::CoreBlockSelector;
use params::CoreTransactionRequest;

use super::{
  connection::SubscriptionId,
  logs::{self, CoreLogFilter},
  operation::Operation,
  protocol,
  subscription::{self, CoreSubscription},
};

#[derive(Deserialize)]
pub(super) enum CoreRequest {
  #[serde(rename = "cfx_getAdmin")]
  Admin(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getSponsorInfo")]
  SponsorInfo(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getStakingBalance")]
  StakingBalance(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getDepositList")]
  DepositList(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getVoteList")]
  VoteList(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getCollateralForStorage")]
  StorageCollateral(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getAccount")]
  Account(RpcAddress, #[serde(default)] Option<EpochNumber>),
  #[serde(rename = "cfx_getStorageRoot")]
  StorageRoot(RpcAddress, #[serde(default)] Option<EpochNumber>),

  #[serde(
    rename = "cfx_getInterestRate",
    deserialize_with = "protocol::deserialize_optional"
  )]
  InterestRate(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getAccumulateInterestRate",
    deserialize_with = "protocol::deserialize_optional"
  )]
  AccumulateInterestRate(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getPoSEconomics",
    deserialize_with = "protocol::deserialize_optional"
  )]
  PosEconomics(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getSupplyInfo",
    deserialize_with = "protocol::deserialize_optional"
  )]
  SupplyInfo(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getCollateralInfo",
    deserialize_with = "protocol::deserialize_optional"
  )]
  CollateralInfo(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getFeeBurnt",
    deserialize_with = "protocol::deserialize_optional"
  )]
  FeeBurnt(Option<EpochNumber>),
  #[serde(
    rename = "cfx_getParamsFromVote",
    deserialize_with = "protocol::deserialize_optional"
  )]
  VoteParams(Option<EpochNumber>),

  #[serde(rename = "cfx_checkBalanceAgainstTransaction")]
  CheckBalance(
    RpcAddress,
    RpcAddress,
    U256,
    U256,
    U256,
    #[serde(default)] Option<EpochNumber>,
  ),

  #[serde(rename = "cfx_clientVersion")]
  ClientVersion,
  #[serde(rename = "cfx_gasPrice")]
  GasPrice,
  #[serde(rename = "cfx_maxPriorityFeePerGas")]
  MaxPriorityFeePerGas,
  #[serde(rename = "cfx_feeHistory")]
  FeeHistory(U64, EpochNumber, #[serde(default)] Option<Vec<f64>>),
  #[serde(rename = "cfx_getBestBlockHash")]
  BestBlockHash,
  #[serde(rename = "cfx_getStatus")]
  Status,
  #[serde(
    rename = "cfx_getConfirmationRiskByHash",
    deserialize_with = "protocol::deserialize_single"
  )]
  ConfirmationRisk(H256),
  #[serde(
    rename = "cfx_getBlocksByEpoch",
    deserialize_with = "protocol::deserialize_single"
  )]
  BlocksByEpoch(EpochNumber),
  #[serde(
    rename = "cfx_getSkippedBlocksByEpoch",
    deserialize_with = "protocol::deserialize_single"
  )]
  SkippedBlocksByEpoch(EpochNumber),
  #[serde(rename = "cfx_getBlockByHash")]
  BlockByHash(H256, bool),
  #[serde(rename = "cfx_getBlockByEpochNumber")]
  BlockByEpoch(EpochNumber, bool),
  #[serde(rename = "cfx_getBlockByBlockNumber")]
  BlockByNumber(U64, bool),
  #[serde(rename = "cfx_getBlockByHashWithPivotAssumption")]
  BlockWithPivot(H256, H256, U64),
  #[serde(
    rename = "cfx_getTransactionByHash",
    deserialize_with = "protocol::deserialize_single"
  )]
  TransactionByHash(H256),
  #[serde(
    rename = "cfx_getTransactionReceipt",
    deserialize_with = "protocol::deserialize_single"
  )]
  TransactionReceipt(H256),
  #[serde(rename = "cfx_getEpochReceipts")]
  EpochReceipts(BlockHashOrEpochNumber, #[serde(default)] Option<bool>),
  #[serde(rename = "cfx_accounts")]
  Accounts,
  #[serde(
    rename = "cfx_sendRawTransaction",
    deserialize_with = "protocol::deserialize_single"
  )]
  SendRawTransaction(Bytes),
  #[serde(rename = "cfx_sendTransaction")]
  SendTransaction(CoreTransactionRequest, #[serde(default)] Option<String>),
  #[serde(rename = "cfx_signTransaction")]
  SignTransaction(CoreTransactionRequest, #[serde(default)] Option<String>),
  #[serde(rename = "cfx_sign")]
  Sign(Bytes, RpcAddress, #[serde(default)] Option<String>),
  #[serde(rename = "cfx_call")]
  Call(
    CoreTransactionRequest,
    #[serde(default)] Option<BlockHashOrEpochNumber>,
  ),
  #[serde(rename = "cfx_estimateGasAndCollateral")]
  Estimate(
    CoreTransactionRequest,
    #[serde(default)] Option<EpochNumber>,
  ),
  #[serde(
    rename = "cfx_epochNumber",
    deserialize_with = "protocol::deserialize_optional"
  )]
  EpochNumber(Option<EpochNumber>),
  #[serde(rename = "cfx_getBalance")]
  Balance(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getNextNonce")]
  Nonce(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getCode")]
  Code(RpcAddress, #[serde(default)] Option<BlockHashOrEpochNumber>),
  #[serde(rename = "cfx_getStorageAt")]
  Storage(
    RpcAddress,
    U256,
    #[serde(default)] Option<BlockHashOrEpochNumber>,
  ),
  #[serde(
    rename = "cfx_getLogs",
    deserialize_with = "protocol::deserialize_single"
  )]
  Logs(CoreLogFilter),
  #[serde(
    rename = "cfx_newFilter",
    deserialize_with = "protocol::deserialize_single"
  )]
  NewFilter(CoreLogFilter),
  #[serde(rename = "cfx_newBlockFilter")]
  NewBlockFilter,
  #[serde(rename = "cfx_newPendingTransactionFilter")]
  NewPendingTransactionFilter,
  #[serde(
    rename = "cfx_getFilterChanges",
    deserialize_with = "protocol::deserialize_single"
  )]
  FilterChanges(H128),
  #[serde(
    rename = "cfx_getFilterLogs",
    deserialize_with = "protocol::deserialize_single"
  )]
  FilterLogs(H128),
  #[serde(
    rename = "cfx_uninstallFilter",
    deserialize_with = "protocol::deserialize_single"
  )]
  UninstallFilter(H128),
  #[serde(rename = "cfx_subscribe")]
  Subscribe(CoreSubscription),
  #[serde(
    rename = "cfx_unsubscribe",
    deserialize_with = "protocol::deserialize_single"
  )]
  Unsubscribe(SubscriptionId),

  #[serde(rename = "txpool_status")]
  PoolStatus,
  #[serde(rename = "txpool_transactionByAddressAndNonce")]
  PoolTransaction(RpcAddress, U256),
  #[serde(
    rename = "txpool_pendingNonceRange",
    deserialize_with = "protocol::deserialize_single"
  )]
  PendingNonceRange(RpcAddress),
  #[serde(
    rename = "txpool_txWithPoolInfo",
    deserialize_with = "protocol::deserialize_single"
  )]
  TransactionPoolInfo(H256),
  #[serde(
    rename = "txpool_accountPendingInfo",
    alias = "cfx_getAccountPendingInfo",
    deserialize_with = "protocol::deserialize_single"
  )]
  AccountPendingInfo(RpcAddress),
  #[serde(
    rename = "txpool_accountPendingTransactions",
    alias = "cfx_getAccountPendingTransactions"
  )]
  AccountPendingTransactions(
    RpcAddress,
    #[serde(default)] Option<U256>,
    #[serde(default)] Option<U64>,
  ),

  #[serde(
    rename = "debug_inspectTxPool",
    deserialize_with = "protocol::deserialize_optional"
  )]
  InspectPool(Option<RpcAddress>),
  #[serde(
    rename = "debug_txPoolContent",
    deserialize_with = "protocol::deserialize_optional"
  )]
  PoolContent(Option<RpcAddress>),
  #[serde(
    rename = "debug_txPoolAccountTransactions",
    deserialize_with = "protocol::deserialize_single"
  )]
  PoolAccountTransactions(RpcAddress),
  #[serde(rename = "debug_clearTxPool")]
  ClearPool,
  #[serde(rename = "debug_currentSyncPhase")]
  CurrentSyncPhase,
  #[serde(
    rename = "debug_getTransactionsByEpoch",
    deserialize_with = "protocol::deserialize_single"
  )]
  DebugTransactionsByEpoch(U64),
  #[serde(
    rename = "debug_getTransactionsByBlock",
    deserialize_with = "protocol::deserialize_single"
  )]
  DebugTransactionsByBlock(H256),

  #[serde(
    rename = "txpool_nextNonce",
    deserialize_with = "protocol::deserialize_single"
  )]
  PendingNonce(RpcAddress),
}

impl CoreRequest {
  pub(super) fn into_operation(self) -> Operation {
    match self {
      Self::Admin(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_admin_at(address, selector),
          state::encode_admin,
        )
      }

      Self::SponsorInfo(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_sponsor_info_at(address, selector),
          state::encode_sponsor_info,
        )
      }

      Self::StakingBalance(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_staking_balance_at(address, selector),
          state::encode_result,
        )
      }

      Self::DepositList(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_deposit_list_at(address, selector),
          state::encode_result,
        )
      }

      Self::VoteList(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_vote_list_at(address, selector),
          state::encode_result,
        )
      }

      Self::StorageCollateral(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_collateral_for_storage_at(address, selector),
          state::encode_result,
        )
      }

      Self::Account(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_account_at(address, selector),
          state::encode_account,
        )
      }

      Self::InterestRate(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_interest_rate_at(selector),
          state::encode_global_result,
        )
      }

      Self::AccumulateInterestRate(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_accumulated_interest_rate_at(selector),
          state::encode_global_result,
        )
      }

      Self::PosEconomics(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_pos_economics_at(selector),
          state::encode_pos_economics,
        )
      }

      Self::SupplyInfo(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_supply_info_at(selector),
          state::encode_supply_info,
        )
      }

      Self::CollateralInfo(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_collateral_info_at(selector),
          state::encode_collateral_info,
        )
      }

      Self::FeeBurnt(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_fee_burnt_at(selector),
          state::encode_global_result,
        )
      }

      Self::VoteParams(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_vote_params_at(selector),
          state::encode_vote_params,
        )
      }

      Self::CheckBalance(account, contract, gas_limit, gas_price, storage_limit, epoch) => {
        let Ok(storage_limit) = u64::try_from(storage_limit) else {
          return Operation::Immediate(Err(protocol::RpcError::invalid_params()));
        };
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| {
            node.core_check_balance_at(
              account,
              contract,
              gas_limit,
              gas_price,
              storage_limit,
              selector,
            )
          },
          state::encode_balance_check,
        )
      }

      Self::StorageRoot(address, epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestState));

        Operation::on_node(
          move |node| node.core_storage_root(address, selector),
          |result| result.map(|root| json!(root)).map_err(error::storage_root),
        )
      }

      Self::ClientVersion => Operation::Immediate(Ok(json!(super::client_version()))),
      Self::GasPrice => Operation::on_node(
        |node| node.core_gas_price(),
        |result| {
          result
            .map(|price| json!(price))
            .map_err(crate::rpc::error::fork_error)
        },
      ),
      Self::MaxPriorityFeePerGas => Operation::on_node(
        |node| node.core_max_priority_fee_per_gas(),
        |result| {
          result
            .map(|price| json!(price))
            .map_err(crate::rpc::error::fork_error)
        },
      ),
      Self::FeeHistory(count, newest, percentiles) => fees::history(count, newest, percentiles),
      Self::BestBlockHash => {
        Operation::on_node(|node| node.best_block_hash(), |hash| Ok(json!(hash)))
      }
      Self::Status => Operation::on_node(
        |node| node.core_status(),
        |status| {
          let ids = status.chain_ids();
          let positions = status.positions();
          Ok(json!(cfx_rpc_cfx_types::Status {
            best_hash: status.best_hash(),
            chain_id: ids.chain_id.into(),
            ethereum_space_chain_id: ids.espace_chain_id.into(),
            network_id: ids.network_id.into(),
            epoch_number: status.epoch_height().into(),
            block_number: status.block_count().into(),
            pending_tx_number: status.pending_transactions().into(),
            latest_state: status.latest_state().into(),
            latest_checkpoint: positions.checkpoint.height.into(),
            latest_confirmed: positions.confirmed.height.into(),
            latest_finalized: positions.finalized.height.into(),
          }))
        },
      ),
      Self::ConfirmationRisk(hash) => Operation::on_node(
        move |node| node.core_confirmation_risk(hash),
        |result| {
          result
            .map(|value| json!(value))
            .map_err(crate::rpc::error::fork_error)
        },
      ),
      Self::BlocksByEpoch(epoch) => {
        let selector = params::epoch_selector(epoch);
        Operation::on_node(
          move |node| node.core_blocks_by_epoch(selector),
          query::encode_epoch_blocks,
        )
      }
      Self::SkippedBlocksByEpoch(epoch) => {
        let selector = params::epoch_selector(epoch);
        Operation::on_node(
          move |node| node.core_skipped_blocks_by_epoch(selector),
          query::encode_epoch_blocks,
        )
      }
      Self::BlockByHash(hash, full) => Operation::on_node(
        move |node| node.core_block(CoreBlockSelector::Hash(hash), full),
        move |result| query::encode_block(result, full),
      ),
      Self::BlockByEpoch(epoch, full) => {
        let selector = CoreBlockSelector::Epoch(params::epoch_selector(epoch));
        Operation::on_node(
          move |node| node.core_block(selector, full),
          move |result| query::encode_block(result, full),
        )
      }
      Self::BlockByNumber(number, full) => Operation::on_node(
        move |node| node.core_block(CoreBlockSelector::Number(number.as_u64()), full),
        move |result| query::encode_block(result, full),
      ),
      Self::BlockWithPivot(hash, pivot, epoch) => Operation::on_node(
        move |node| node.core_block_with_pivot(hash, pivot, epoch.as_u64()),
        query::encode_block_with_pivot,
      ),
      Self::TransactionByHash(hash) => Operation::on_node(
        move |node| node.core_transaction_by_hash(hash),
        query::encode_transaction,
      ),
      Self::TransactionReceipt(hash) => Operation::on_node(
        move |node| node.core_transaction_receipt(hash),
        query::encode_receipt,
      ),
      Self::EpochReceipts(selector, include_espace) => {
        let selector = params::state_selector(Some(selector));
        let include_espace = include_espace.unwrap_or(false);
        Operation::on_node(
          move |node| node.core_epoch_receipts(selector, include_espace),
          move |result| query::encode_epoch_receipts(result, include_espace),
        )
      }
      Self::Accounts => Operation::on_node(
        |node| node.core_accounts(),
        |(accounts, network)| {
          Ok(json!(
            accounts
              .into_iter()
              .map(|address| convert::address(address, network))
              .collect::<Vec<_>>()
          ))
        },
      ),
      Self::SendRawTransaction(raw) => Operation::on_node(
        move |node| node.core_send_raw_transaction(&raw.0),
        |result| {
          result
            .map(|hash| json!(hash))
            .map_err(error::raw_transaction)
        },
      ),
      Self::SendTransaction(request, _password) => Operation::on_node(
        move |node| node.core_send_transaction(request.into()),
        |result| result.map(|hash| json!(hash)).map_err(error::transaction),
      ),
      Self::SignTransaction(request, _password) => Operation::on_node(
        move |node| node.core_sign_transaction(request.into()),
        |result| {
          result
            .map(|raw| json!(Bytes::new(raw)))
            .map_err(error::transaction)
        },
      ),
      Self::Sign(data, address, _password) => Operation::on_node(
        move |node| node.core_sign_message(address, &data.0),
        |result| {
          result
            .map(|signature| json!(H520::from(signature)))
            .map_err(error::signing)
        },
      ),
      Self::Call(request, selector) => {
        let selector = params::state_selector(selector);
        Operation::on_node(
          move |node| node.core_call(request.into(), selector),
          |result| {
            result
              .map(|output| json!(Bytes::new(output)))
              .map_err(error::call)
          },
        )
      }
      Self::Estimate(request, epoch) => {
        let selector = crate::runtime::StateSelector::Epoch(params::epoch_selector(
          epoch.unwrap_or(EpochNumber::LatestState),
        ));
        Operation::on_node(
          move |node| node.core_estimate(request.into(), selector),
          |result| {
            result
              .map(|estimate| {
                json!(EstimateGasAndCollateralResponse {
                  gas_limit: estimate.gas_limit,
                  // Core reports the suggested gas limit in both gas fields.
                  gas_used: estimate.gas_limit,
                  storage_collateralized: estimate.storage_limit.into(),
                })
              })
              .map_err(error::estimate)
          },
        )
      }
      Self::EpochNumber(epoch) => {
        let selector = params::epoch_selector(epoch.unwrap_or(EpochNumber::LatestMined));
        Operation::on_node(
          move |node| node.core_epoch_number(selector),
          |height| {
            height
              .map(|height| json!(U256::from(height)))
              .ok_or_else(protocol::RpcError::invalid_params)
          },
        )
      }
      Self::Balance(address, selector) => {
        let selector = params::state_selector(selector);
        Operation::on_node(
          move |node| node.core_balance_at(address, selector),
          state::encode_result,
        )
      }
      Self::Nonce(address, selector) => {
        let selector = params::state_selector(selector);
        Operation::on_node(
          move |node| node.core_nonce_at(address, selector),
          state::encode_result,
        )
      }
      Self::Code(address, selector) => {
        let selector = params::state_selector(selector);
        Operation::on_node(
          move |node| node.core_code_at(address, selector),
          state::encode_code,
        )
      }
      Self::Storage(address, position, selector) => {
        let selector = params::state_selector(selector);
        let position = H256::from_uint(&position);
        Operation::on_node(
          move |node| node.core_storage_at(address, position, selector),
          state::encode_result,
        )
      }
      Self::Logs(filter) => logs::get_logs(Space::Native, filter.into_input()),
      Self::NewFilter(filter) => logs::new_filter(Space::Native, filter.into_input()),
      Self::NewBlockFilter => Operation::on_node(
        |node| node.new_block_filter(Space::Native),
        |id| Ok(json!(id)),
      ),
      Self::NewPendingTransactionFilter => Operation::on_node(
        |node| node.new_pending_transaction_filter(PendingTransactionFilter::Hashes(Space::Native)),
        |id| Ok(json!(id)),
      ),
      Self::FilterChanges(id) => logs::filter_changes(Space::Native, id),
      Self::FilterLogs(id) => logs::filter_logs(Space::Native, id),
      Self::UninstallFilter(id) => Operation::on_node(
        move |node| node.uninstall_filter(Space::Native, id),
        |removed| Ok(json!(removed)),
      ),
      Self::Subscribe(request) => request.into_operation(),
      Self::Unsubscribe(id) => subscription::unsubscribe(id),

      Self::PoolStatus => Operation::on_node(|node| node.core_pool_status(), pool::status),

      Self::PoolTransaction(address, nonce) => Operation::on_node(
        move |node| node.core_pool_transaction_at_nonce(address, nonce),
        |result| {
          let (transaction, network) = result.map_err(error::invalid)?;

          Ok(json!(transaction.map(|transaction| {
            convert::pending_transaction(&transaction, network)
          })))
        },
      ),

      Self::PendingNonceRange(address) => Operation::on_node(
        move |node| node.core_pending_nonce_range(address),
        pool::nonce_range,
      ),

      Self::TransactionPoolInfo(hash) => Operation::on_node(
        move |node| node.core_pool_transaction_info(hash),
        pool::transaction_info,
      ),

      Self::AccountPendingInfo(address) => Operation::on_node(
        move |node| node.core_pool_account_info(address),
        pool::account_info,
      ),

      Self::AccountPendingTransactions(address, start_nonce, limit) => {
        let limit = limit.map_or(usize::MAX, |limit| {
          usize::try_from(limit.as_u64()).unwrap_or(usize::MAX)
        });

        Operation::on_node(
          move |node| node.core_pool_account_transactions(address, start_nonce, limit),
          pool::account_transactions,
        )
      }

      Self::InspectPool(address) => Operation::on_node(
        move |node| node.core_pool_content(address),
        |result| pool::content(result, true),
      ),

      Self::PoolContent(address) => Operation::on_node(
        move |node| node.core_pool_content(address),
        |result| pool::content(result, false),
      ),

      Self::PoolAccountTransactions(address) => Operation::on_node(
        move |node| node.core_pool_content(Some(address)),
        pool::debug_account_transactions,
      ),

      Self::ClearPool => Operation::on_node(|node| node.drop_all_transactions(), |_| Ok(json!(()))),

      Self::CurrentSyncPhase => Operation::Immediate(Ok(json!("NormalSyncPhase"))),

      Self::DebugTransactionsByEpoch(epoch) => Operation::on_node(
        move |node| node.core_debug_epoch(epoch.as_u64()),
        query::encode_debug_epoch,
      ),

      Self::DebugTransactionsByBlock(hash) => Operation::on_node(
        move |node| node.core_debug_block(hash),
        query::encode_debug_block,
      ),

      Self::PendingNonce(address) => Operation::on_node(
        move |node| node.native_pending_nonce(address),
        query::encode_pending_nonce,
      ),
    }
  }
}
