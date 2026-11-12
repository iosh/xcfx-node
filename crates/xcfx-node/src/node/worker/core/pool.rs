use cfx_addr::Network;
use cfx_rpc_cfx_types::{RpcAddress, address::UnexpectedRpcAddressNetwork};
use cfx_statedb::Result as StateResult;
use cfx_types::{AddressSpaceUtil, H256, U256};

use crate::{
  logs::network,
  runtime::{
    AccountPoolTransactions, CorePoolStatus, PoolAccountInfo, PoolContent, PoolQueryScope,
    PoolTransactionInfo,
  },
  runtime_transaction::RuntimeTransaction,
};

use super::{CoreStateQueryError, NodeCall};

impl NodeCall<'_, '_> {
  pub(crate) fn core_pool_status(self) -> StateResult<CorePoolStatus> {
    self.worker.node.runtime().core_pool_status()
  }

  pub(crate) fn core_pool_transaction_at_nonce(
    self,
    address: RpcAddress,
    nonce: U256,
  ) -> Result<(Option<RuntimeTransaction>, Network), UnexpectedRpcAddressNetwork> {
    let address = self.native_address(address)?;

    let runtime = self.worker.node.runtime();
    let transaction = runtime
      .pool_transaction_at_nonce(address.with_native_space(), nonce)
      .cloned();

    Ok((transaction, network(runtime.network_id())))
  }

  pub(crate) fn core_pending_nonce_range(
    self,
    address: RpcAddress,
  ) -> Result<Option<(U256, U256)>, UnexpectedRpcAddressNetwork> {
    let address = self.native_address(address)?;

    Ok(
      self
        .worker
        .node
        .runtime()
        .pool_pending_nonce_range(address.with_native_space()),
    )
  }

  pub(crate) fn core_pool_account_info(
    self,
    address: RpcAddress,
  ) -> Result<PoolAccountInfo, CoreStateQueryError<cfx_statedb::Error>> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;

    Ok(
      self
        .worker
        .node
        .runtime()
        .pool_account_info(address.with_native_space())?,
    )
  }

  pub(crate) fn core_pool_transaction_info(
    self,
    hash: H256,
  ) -> StateResult<Option<PoolTransactionInfo>> {
    self.worker.node.runtime().pool_transaction_info(hash)
  }

  pub(crate) fn core_pool_account_transactions(
    self,
    address: RpcAddress,
    start_nonce: Option<U256>,
    limit: usize,
  ) -> Result<(AccountPoolTransactions, Network), CoreStateQueryError<cfx_statedb::Error>> {
    let address = self
      .native_address(address)
      .map_err(CoreStateQueryError::Network)?;

    let runtime = self.worker.node.runtime();
    let transactions =
      runtime.pool_account_transactions(address.with_native_space(), start_nonce, limit)?;

    Ok((transactions, network(runtime.network_id())))
  }

  pub(crate) fn core_pool_content(
    self,
    address: Option<RpcAddress>,
  ) -> Result<(PoolContent, Network), CoreStateQueryError<cfx_statedb::Error>> {
    let scope = match address {
      None => PoolQueryScope::All,
      Some(address) => {
        let address = self
          .native_address(address)
          .map_err(CoreStateQueryError::Network)?;

        PoolQueryScope::Account(address.with_native_space())
      }
    };

    let runtime = self.worker.node.runtime();
    let content = runtime.pool_content(scope)?;

    Ok((content, network(runtime.network_id())))
  }

  pub(crate) fn drop_all_transactions(self) {
    self.worker.node.runtime_mut().drop_all_transactions();
  }
}
