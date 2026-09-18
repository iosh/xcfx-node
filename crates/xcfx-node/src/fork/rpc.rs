//! Conflux RPC reads and state encoding at a fixed fork base.

use std::{collections::HashSet, sync::Arc};

use alloy_provider::Provider;
use alloy_rpc_types_eth::BlockId;
use cfx_addr::Network;
use cfx_parameters::{
  consensus::DEFERRED_STATE_EPOCH_COUNT,
  internal_contract_addresses::{
    PARAMS_CONTROL_CONTRACT_ADDRESS, STORAGE_INTEREST_STAKING_CONTRACT_ADDRESS,
    SYSTEM_STORAGE_ADDRESS,
  },
  staking::DRIPS_PER_STORAGE_COLLATERAL_UNIT,
};
use cfx_rpc_cfx_types::{Block, BlockHashOrEpochNumber, EpochNumber, RpcAddress};
use cfx_statedb::global_params::*;
use cfx_types::{Address, AddressUtil, AddressWithSpace, H256, Space, U256};
use keccak_hash::{KECCAK_EMPTY, keccak};
use primitives::{
  Account, CodeInfo, DepositList, SponsorInfo, StorageKey, StorageKeyWithSpace, StorageValue,
  VoteStakeList, account::StoragePoints, storage::STORAGE_LAYOUT_REGULAR_V0,
};

use crate::{execution::ExecutionCommitment, rpc_client::ConfluxRpcClient};

use super::{ForkBase, ForkEpoch, ForkEpochReceipts, ForkLoadError, ForkReadError, load_fork_base};

pub(crate) struct ForkRpc {
  pub(super) base: Arc<ForkBase>,
  rpc: ConfluxRpcClient,
}

impl ForkRpc {
  pub(crate) async fn connect(
    rpc: ConfluxRpcClient,
    epoch: ForkEpoch,
  ) -> Result<Self, ForkLoadError> {
    let base = load_fork_base(&rpc, epoch).await?;
    Ok(Self {
      base: Arc::new(base),
      rpc,
    })
  }

  pub(crate) fn base(&self) -> &ForkBase {
    &self.base
  }

  pub(super) async fn balance(&self, address: AddressWithSpace) -> Result<U256, ForkReadError> {
    match address.space {
      Space::Native => self
        .rpc
        .cfx_getBalance(self.core_address(address.address), self.core_block())
        .await
        .map_err(ForkReadError::from),
      Space::Ethereum => {
        let value = self
          .rpc
          .espace()
          .get_balance(address.address.0.into())
          .block_id(self.espace_block())
          .await?;
        Ok(U256::from_big_endian(&value.to_be_bytes::<32>()))
      }
    }
  }

  /// Validates both sides of compound numeric reads before returning a value.
  /// This detects visible pivot changes; it is not an atomic remote snapshot.
  pub(super) async fn state_value(
    &self,
    key: StateKey,
  ) -> Result<Option<Arc<[u8]>>, ForkReadError> {
    // Hash-based state methods already require the fixed pivot. Numeric RPCs
    // need uncached checks around the entire conversion, including its helpers.
    let numeric_selector = matches!(
      key,
      StateKey::Account(AddressWithSpace {
        space: Space::Native,
        ..
      }) | StateKey::StorageLayout(AddressWithSpace {
        space: Space::Native,
        ..
      }) | StateKey::DepositList(_)
        | StateKey::VoteList(_)
    );
    if numeric_selector {
      self.validate_base().await?;
    }
    let value = match key {
      StateKey::Account(address) => self.account(address).await?,
      StateKey::Code { address, hash } => self.code(address, hash).await?,
      StateKey::Storage { address, slot } => self.storage(address, slot).await?,
      StateKey::StorageLayout(address) => self.storage_layout(address).await?,
      StateKey::DepositList(address) => {
        let entries = self
          .rpc
          .cfx_getDepositList(self.core_address(address), self.epoch())
          .await?;
        (!entries.is_empty()).then(|| encode(&DepositList(entries)))
      }
      StateKey::VoteList(address) => {
        let entries = self
          .rpc
          .cfx_getVoteList(self.core_address(address), self.epoch())
          .await?;
        (!entries.is_empty()).then(|| encode(&VoteStakeList(entries)))
      }
    };
    if numeric_selector {
      self.validate_base().await?;
    }
    Ok(value)
  }

  /// Required initial state, read before the service starts and retained by the
  /// initial state version. Numeric RPCs must stay on the same fixed pivot.
  pub(crate) async fn global_parameters(
    &self,
  ) -> Result<[U256; TOTAL_GLOBAL_PARAMS], ForkReadError> {
    self.validate_base().await?;
    let interest = self.rpc.cfx_getInterestRate(self.epoch()).await?;
    let accumulated = self.rpc.cfx_getAccumulateInterestRate(self.epoch()).await?;
    let supply = self.rpc.cfx_getSupplyInfo(self.epoch()).await?;
    let pos = self.rpc.cfx_getPoSEconomics(self.epoch()).await?;
    let vote = self.rpc.cfx_getParamsFromVote(self.epoch()).await?;
    let burnt = self.rpc.cfx_getFeeBurnt(self.epoch()).await?;
    let collateral = self.rpc.cfx_getCollateralInfo(self.epoch()).await?;
    if collateral.total_storage_tokens != supply.total_collateral {
      return Err(ForkReadError::Inconsistent("global storage collateral"));
    }
    let mut values = [U256::zero(); TOTAL_GLOBAL_PARAMS];
    values[InterestRate::ID] = interest;
    values[AccumulateInterestRate::ID] = accumulated;
    values[TotalIssued::ID] = supply.total_issued;
    values[TotalStaking::ID] = supply.total_staking;
    values[TotalStorage::ID] = supply.total_collateral;
    values[TotalEvmToken::ID] = supply.total_espace_tokens;
    values[UsedStoragePoints::ID] = points_upper_bound(collateral.used_storage_points)?;
    values[ConvertedStoragePoints::ID] = points_upper_bound(collateral.converted_storage_points)?;
    values[TotalPosStaking::ID] = pos.total_pos_staking_tokens;
    values[DistributablePoSInterest::ID] = pos.distributable_pos_interest;
    values[LastDistributeBlock::ID] = pos.last_distribute_block.as_u64().into();
    values[PowBaseReward::ID] = vote.pow_base_reward;
    values[TotalBurnt1559::ID] = burnt;
    values[BaseFeeProp::ID] = vote.base_fee_share_prop;
    self.validate_base().await?;
    Ok(values)
  }

  pub(super) async fn pivot(&self, epoch: u64) -> Result<Arc<Block>, ForkReadError> {
    self.validate_history(epoch).await?;
    let pivot = self.load_historical_pivot(epoch).await?;
    self.validate_base().await?;
    Ok(Arc::new(pivot))
  }

  pub(super) async fn block(&self, epoch: u64, hash: H256) -> Result<Arc<Block>, ForkReadError> {
    self.validate_history(epoch).await?;
    let pivot = self.load_historical_pivot(epoch).await?;
    let block = self
      .rpc
      .cfx_getBlockByHashWithPivotAssumption(hash, pivot.hash, epoch.into())
      .await?;
    if block.hash != hash || block.epoch_number != Some(epoch.into()) {
      return Err(ForkReadError::Inconsistent("block execution identity"));
    }
    self.check_block_number(&block)?;
    self.validate_base().await?;
    Ok(Arc::new(block))
  }

  pub(super) async fn block_hashes(&self, epoch: u64) -> Result<Arc<[H256]>, ForkReadError> {
    self.validate_history(epoch).await?;
    let pivot = self.load_historical_pivot(epoch).await?;
    let hashes = self.load_block_hashes(epoch, pivot.hash).await?;
    self.validate_base().await?;
    Ok(hashes.into())
  }

  pub(super) async fn receipts(
    &self,
    epoch: u64,
    include_espace: bool,
  ) -> Result<Arc<ForkEpochReceipts>, ForkReadError> {
    self.validate_history(epoch).await?;
    let pivot = self.load_historical_pivot(epoch).await?;
    let block_hashes = self.load_block_hashes(epoch, pivot.hash).await?;
    let receipts = self
      .rpc
      .cfx_getEpochReceipts(core_hash_selector(pivot.hash), include_espace)
      .await?
      .ok_or(ForkReadError::Unavailable("epoch receipts"))?;
    if receipts.len() != block_hashes.len()
      || receipts.iter().zip(&block_hashes).any(|(receipts, hash)| {
        receipts
          .iter()
          .any(|receipt| receipt.block_hash != *hash || receipt.epoch_number != Some(epoch.into()))
      })
    {
      return Err(ForkReadError::Inconsistent("epoch receipt identity"));
    }
    self.validate_base().await?;
    Ok(Arc::new(ForkEpochReceipts {
      pivot_hash: pivot.hash,
      block_hashes,
      receipts,
    }))
  }

  pub(super) async fn commitment(&self, epoch: u64) -> Result<ExecutionCommitment, ForkReadError> {
    self.validate_history(epoch).await?;
    // Witness headers can be above the base; their execution position cannot.
    let witness_height = epoch
      .checked_add(DEFERRED_STATE_EPOCH_COUNT)
      .ok_or(ForkReadError::OutOfRange("commitment witness height"))?;
    let witness = self.load_pivot(witness_height).await?;
    if !witness.blame.is_zero() {
      return Err(ForkReadError::Unsupported(
        "deferred commitment with nonzero blame",
      ));
    }
    self.validate_base().await?;
    Ok(ExecutionCommitment {
      state_root: Some(witness.deferred_state_root),
      receipts_root: witness.deferred_receipts_root,
      logs_bloom_hash: witness.deferred_logs_bloom_hash,
    })
  }

  async fn validate_history(&self, epoch: u64) -> Result<(), ForkReadError> {
    assert!(
      epoch <= self.base.epoch_height,
      "remote history epoch {epoch} exceeds fork base {}",
      self.base.epoch_height,
    );
    self.validate_base().await
  }

  async fn validate_base(&self) -> Result<(), ForkReadError> {
    let pivot = self.load_pivot(self.base.epoch_height).await?;
    if pivot.hash != self.base.pivot_hash
      || pivot.block_number != Some(self.base.pivot_block_number.into())
    {
      return Err(ForkReadError::Inconsistent(
        "fixed pivot or cumulative block number changed",
      ));
    }
    Ok(())
  }

  fn epoch(&self) -> EpochNumber {
    EpochNumber::Num(self.base.epoch_height.into())
  }

  fn core_block(&self) -> BlockHashOrEpochNumber {
    core_hash_selector(self.base.pivot_hash)
  }

  fn espace_block(&self) -> BlockId {
    BlockId::hash_canonical(self.base.pivot_hash.0.into())
  }

  fn core_address(&self, address: Address) -> RpcAddress {
    let network = match self.base.network.network_id {
      1 => Network::Test,
      1029 => Network::Main,
      id => Network::Id(id),
    };
    RpcAddress::try_from_h160(address, network)
      .expect("a 20-byte address and normalized network must be encodable")
  }

  async fn load_pivot(&self, epoch: u64) -> Result<Block, ForkReadError> {
    self
      .rpc
      .cfx_getBlockByEpochNumber(EpochNumber::Num(epoch.into()))
      .await?
      .ok_or(ForkReadError::Unavailable("pivot header"))
  }

  async fn load_historical_pivot(&self, epoch: u64) -> Result<Block, ForkReadError> {
    let pivot = self.load_pivot(epoch).await?;
    self.check_block_number(&pivot)?;
    Ok(pivot)
  }

  fn check_block_number(&self, block: &Block) -> Result<(), ForkReadError> {
    let number = block
      .block_number
      .ok_or(ForkReadError::Unavailable("Core blockNumber"))?;
    if number > self.base.pivot_block_number.into() {
      return Err(ForkReadError::Inconsistent(
        "historical block number exceeds fork base",
      ));
    }
    Ok(())
  }

  async fn account(&self, address: AddressWithSpace) -> Result<Option<Arc<[u8]>>, ForkReadError> {
    let mut account = Account::new_empty(&address);
    if address.space == Space::Ethereum {
      account.balance = self.balance(address).await?;
      // Conflux stores a U256 nonce; Provider::get_transaction_count narrows to u64.
      account.nonce = self
        .rpc
        .espace()
        .raw_request::<_, U256>(
          "eth_getTransactionCount".into(),
          (address.address, self.espace_block()),
        )
        .await?;
      let code = self.code_bytes(address).await?;
      if account.balance.is_zero() && account.nonce.is_zero() && code.is_empty() {
        return Ok(None);
      }
      account.code_hash = keccak(&code);
      return Ok(Some(encode(&account)));
    }

    let rpc_address = self.core_address(address.address);
    let admin = self
      .rpc
      .cfx_getAdmin(rpc_address.clone(), self.epoch())
      .await?;
    let Some(admin) = admin else { return Ok(None) };
    let remote = self
      .rpc
      .cfx_getAccount(rpc_address.clone(), self.epoch())
      .await?;
    if remote.address.hex_address != address.address
      || remote.admin.hex_address != admin.hex_address
    {
      return Err(ForkReadError::Inconsistent(
        "Core account identity or admin",
      ));
    }
    let collateral = self
      .rpc
      .cfx_getCollateralForStorage(rpc_address.clone(), self.epoch())
      .await?;
    let used_points = remote
      .collateral_for_storage
      .checked_sub(collateral)
      .ok_or(ForkReadError::Inconsistent(
        "account collateral is below token collateral",
      ))?;
    account.balance = remote.balance;
    account.nonce = remote.nonce;
    account.code_hash = remote.code_hash;
    account.staking_balance = remote.staking_balance;
    account.collateral_for_storage = collateral;
    account.accumulated_interest_return = remote.accumulated_interest_return;
    account.admin = admin.hex_address;

    if address.address.is_contract_address()
      || (account.code_hash != KECCAK_EMPTY && !account.code_hash.is_zero())
    {
      let sponsor = self
        .rpc
        .cfx_getSponsorInfo(rpc_address, self.epoch())
        .await?;
      let unit = *DRIPS_PER_STORAGE_COLLATERAL_UNIT;
      if sponsor.used_storage_points != used_points / unit {
        return Err(ForkReadError::Inconsistent("account used storage points"));
      }
      let unused = sponsor
        .available_storage_points
        .checked_mul(unit)
        .ok_or(ForkReadError::OutOfRange("available storage points"))?;
      if unused.is_zero() && used_points.is_zero() {
        return Err(ForkReadError::Unsupported(
          "Core sponsor storage-points initialization is not exposed by RPC",
        ));
      }
      account.sponsor_info = SponsorInfo {
        sponsor_for_gas: sponsor.sponsor_for_gas.hex_address,
        sponsor_for_collateral: sponsor.sponsor_for_collateral.hex_address,
        sponsor_gas_bound: sponsor.sponsor_gas_bound,
        sponsor_balance_for_gas: sponsor.sponsor_balance_for_gas,
        sponsor_balance_for_collateral: sponsor.sponsor_balance_for_collateral,
        // Spendable points use the RPC lower bound; never add global compensation.
        storage_points: Some(StoragePoints {
          unused,
          used: used_points,
        }),
      };
    } else if !used_points.is_zero() {
      return Err(ForkReadError::Inconsistent(
        "storage points on a basic Core account",
      ));
    }
    Ok(Some(encode(&account)))
  }

  async fn code_bytes(&self, address: AddressWithSpace) -> Result<Vec<u8>, ForkReadError> {
    match address.space {
      Space::Native => Ok(
        self
          .rpc
          .cfx_getCode(self.core_address(address.address), self.core_block())
          .await?
          .into_vec(),
      ),
      Space::Ethereum => Ok(
        self
          .rpc
          .espace()
          .get_code_at(address.address.0.into())
          .block_id(self.espace_block())
          .await?
          .to_vec(),
      ),
    }
  }

  async fn code(
    &self,
    address: AddressWithSpace,
    hash: H256,
  ) -> Result<Option<Arc<[u8]>>, ForkReadError> {
    let code = self.code_bytes(address).await?;
    if keccak(&code) != hash {
      return Err(ForkReadError::Inconsistent(
        "code hash does not match requested record",
      ));
    }
    if code.is_empty() {
      return Ok(None);
    }
    if address.space == Space::Native {
      return Err(ForkReadError::Unsupported(
        "Core code owner is not exposed by RPC",
      ));
    }
    Ok(Some(encode(&CodeInfo {
      code: Arc::new(code),
      owner: Address::zero(),
    })))
  }

  async fn storage(
    &self,
    address: AddressWithSpace,
    slot: H256,
  ) -> Result<Option<Arc<[u8]>>, ForkReadError> {
    let slot_number = U256::from_big_endian(slot.as_bytes());
    let value = match address.space {
      Space::Native => self
        .rpc
        .cfx_getStorageAt(
          self.core_address(address.address),
          slot_number,
          self.core_block(),
        )
        .await?
        .unwrap_or_default(),
      Space::Ethereum => {
        self
          .rpc
          .espace()
          .raw_request::<_, H256>(
            "eth_getStorageAt".into(),
            (address.address, slot_number, self.espace_block()),
          )
          .await?
      }
    };
    if value.is_zero() {
      return Ok(None);
    }
    if address.space == Space::Native && address.address != SYSTEM_STORAGE_ADDRESS {
      return Err(ForkReadError::Unsupported(
        "Core storage owner is not exposed by RPC",
      ));
    }
    Ok(Some(encode(&StorageValue {
      value: U256::from_big_endian(value.as_bytes()),
      owner: None,
    })))
  }

  async fn storage_layout(
    &self,
    address: AddressWithSpace,
  ) -> Result<Option<Arc<[u8]>>, ForkReadError> {
    let exists = match address.space {
      // These namespaces hold protocol state independently of account bytecode.
      Space::Native
        if address.address == SYSTEM_STORAGE_ADDRESS
          || address.address == STORAGE_INTEREST_STAKING_CONTRACT_ADDRESS
          || address.address == PARAMS_CONTROL_CONTRACT_ADDRESS =>
      {
        true
      }
      Space::Native => self
        .rpc
        .cfx_getAdmin(self.core_address(address.address), self.epoch())
        .await?
        .is_some(),
      Space::Ethereum => self.account(address).await?.is_some(),
    };
    // This is local adapter metadata, not an imported remote MPT layout or proof.
    Ok(exists.then(|| STORAGE_LAYOUT_REGULAR_V0.to_bytes().into()))
  }

  async fn load_block_hashes(&self, epoch: u64, pivot: H256) -> Result<Vec<H256>, ForkReadError> {
    let hashes = self
      .rpc
      .cfx_getBlocksByEpoch(EpochNumber::Num(epoch.into()))
      .await?;
    let mut unique = HashSet::with_capacity(hashes.len());
    if hashes.last() != Some(&pivot) || hashes.iter().any(|hash| !unique.insert(*hash)) {
      return Err(ForkReadError::Inconsistent("ordered epoch block set"));
    }
    Ok(hashes)
  }
}

/// Owned keys for state entries exposed by the remote RPC methods.
/// Every key reads an optional, encoded Conflux state entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum StateKey {
  Account(AddressWithSpace),
  Code {
    address: AddressWithSpace,
    hash: H256,
  },
  Storage {
    address: AddressWithSpace,
    slot: H256,
  },
  StorageLayout(AddressWithSpace),
  DepositList(Address),
  VoteList(Address),
}

impl TryFrom<StorageKeyWithSpace<'_>> for StateKey {
  type Error = ForkReadError;

  fn try_from(key: StorageKeyWithSpace<'_>) -> Result<Self, Self::Error> {
    let key = match key.key {
      StorageKey::AccountKey(bytes) => Self::Account(account_address(bytes, key.space)),
      StorageKey::StorageRootKey(bytes) => Self::StorageLayout(account_address(bytes, key.space)),
      StorageKey::StorageKey {
        address_bytes,
        storage_key,
      } => {
        if storage_key.len() != 32 {
          return Err(ForkReadError::Unsupported(
            "state key is not exposed by a slot RPC",
          ));
        }
        Self::Storage {
          address: account_address(address_bytes, key.space),
          slot: H256::from_slice(storage_key),
        }
      }
      StorageKey::CodeKey {
        address_bytes,
        code_hash_bytes,
      } => {
        assert_eq!(
          code_hash_bytes.len(),
          32,
          "executor code key must contain a full hash"
        );
        Self::Code {
          address: account_address(address_bytes, key.space),
          hash: H256::from_slice(code_hash_bytes),
        }
      }
      StorageKey::DepositListKey(bytes) if key.space == Space::Native => {
        Self::DepositList(account_address(bytes, key.space).address)
      }
      StorageKey::VoteListKey(bytes) if key.space == Space::Native => {
        Self::VoteList(account_address(bytes, key.space).address)
      }
      _ => {
        return Err(ForkReadError::Unsupported(
          "point read of a state namespace",
        ));
      }
    };
    Ok(key)
  }
}

fn account_address(bytes: &[u8], space: Space) -> AddressWithSpace {
  assert_eq!(
    bytes.len(),
    20,
    "executor state key must contain a full account address"
  );
  AddressWithSpace {
    address: Address::from_slice(bytes),
    space,
  }
}

fn points_upper_bound(points: U256) -> Result<U256, ForkReadError> {
  let unit = *DRIPS_PER_STORAGE_COLLATERAL_UNIT;
  let lower = points
    .checked_mul(unit)
    .ok_or(ForkReadError::OutOfRange("global storage points"))?;
  Ok(lower.checked_add(unit - U256::one()).unwrap_or(U256::MAX))
}

fn encode<T: rlp::Encodable>(value: &T) -> Arc<[u8]> {
  rlp::encode(value).as_ref().into()
}

fn core_hash_selector(hash: H256) -> BlockHashOrEpochNumber {
  // Conflux treats the legacy hash selector as require-pivot. Its optional-object
  // serializer is not compatible with the server at the pinned revision.
  BlockHashOrEpochNumber::BlockHashWithOption {
    hash,
    require_pivot: None,
  }
}
