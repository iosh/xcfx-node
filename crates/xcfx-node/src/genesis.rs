//! Builds and executes the local Conflux genesis transition.

use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::{
  executive::{ExecutionOutcome, ExecutiveContext, TransactOptions},
  internal_contract::initialize_internal_contract_accounts,
  machine::Machine,
  state::State,
};
use cfx_internal_common::StateRootWithAuxInfo;
use cfx_parameters::{
  consensus::{GENESIS_GAS_LIMIT, ONE_CFX_IN_DRIP},
  consensus_internal::{GENESIS_TOKEN_COUNT_IN_CFX, TWO_YEAR_UNLOCK_TOKEN_COUNT_IN_CFX},
  genesis::{
    GENESIS_ACCOUNT_ADDRESS, GENESIS_TRANSACTION_CREATE_CREATE2FACTORY,
    GENESIS_TRANSACTION_CREATE_FUND_POOL,
    GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_FOUR_YEAR_UNLOCK,
    GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_TWO_YEAR_UNLOCK, GENESIS_TRANSACTION_DATA_STR,
  },
};
use cfx_statedb::StateDb;
use cfx_types::{
  Address, AddressSpaceUtil, AddressWithSpace, CreateContractAddressType, Space, U256,
  cal_contract_address_with_space,
};
use cfx_vm_types::Env;
use primitives::{
  Action, Block, BlockHeaderBuilder, SignedTransaction,
  transaction::native_transaction::NativeTransaction,
};
use rustc_hex::FromHex;
use thiserror::Error;

use crate::{
  mpt::indexed_mpt_root,
  state::{
    layered_mpt_state::LayeredMptState,
    state_version::{CommittedStateVersion, StateCandidate, StateVersion},
  },
};

const GENESIS_CONTRACT_NAMES: [&str; 7] = [
  "CREATE2FACTORY",
  "TWO_YEAR_UNLOCK",
  "FOUR_YEAR_UNLOCK",
  "INVESTOR_FUND",
  "TEAM_FUND",
  "ECO_FUND",
  "COMMUNITY_FUND",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GenesisHeaderInput {
  pub(crate) author: Address,
  pub(crate) difficulty: U256,
}

pub(crate) struct ExecutedGenesis {
  pub(crate) block: Block,
  pub(crate) state_root: StateRootWithAuxInfo,
  pub(crate) committed_state: CommittedStateVersion,
}

#[derive(Debug, Error)]
pub(crate) enum GenesisError {
  #[error(transparent)]
  State(#[from] cfx_statedb::Error),

  #[error(
    "Genesis contract deployment transaction {index} ({contract}) did not finish: {outcome:?}"
  )]
  DeploymentDidNotFinish {
    index: usize,
    contract: &'static str,
    outcome: ExecutionOutcome,
  },
}

pub(crate) fn execute_genesis(
  machine: Arc<Machine>,
  allocations: BTreeMap<AddressWithSpace, U256>,
  header: GenesisHeaderInput,
) -> Result<ExecutedGenesis, GenesisError> {
  let parent = Arc::new(StateVersion::genesis_parent());

  let candidate = StateCandidate::new(parent);

  let (backend, committed_state_receiver) = LayeredMptState::new(candidate);

  let db = StateDb::new(Box::new(backend));
  let mut state = State::new(db)?;

  initialize_internal_contract_accounts(
    &mut state,
    machine.internal_contracts().initialized_at_genesis(),
  )?;

  for (address, balance) in allocations {
    state.add_balance(&address, &balance)?;
    state.add_total_issued(balance);

    if address.space == Space::Ethereum {
      state.add_total_evm_tokens(balance);
    }
  }
  let genesis_token_count = U256::from(GENESIS_TOKEN_COUNT_IN_CFX) * U256::from(ONE_CFX_IN_DRIP);
  let two_year_unlock_token_count =
    U256::from(TWO_YEAR_UNLOCK_TOKEN_COUNT_IN_CFX) * U256::from(ONE_CFX_IN_DRIP);
  let four_year_unlock_token_count = genesis_token_count - two_year_unlock_token_count;

  state.add_total_issued(genesis_token_count);

  let genesis_account = GENESIS_ACCOUNT_ADDRESS.with_native_space();
  let genesis_account_balance = U256::from(ONE_CFX_IN_DRIP) * U256::from(100) + genesis_token_count;

  state.add_balance(&genesis_account, &genesis_account_balance)?;
  state.commit_cache(false);

  let chain_id = machine.params().chain_id(0, Space::Native);
  let transactions = build_genesis_transactions(
    chain_id,
    two_year_unlock_token_count,
    four_year_unlock_token_count,
  );

  for (index, transaction) in transactions.iter().enumerate().skip(1) {
    let contract = GENESIS_CONTRACT_NAMES[index - 1];

    execute_genesis_deployment(index, contract, transaction, &mut state, machine.as_ref())?;

    let (contract_address, _) = cal_contract_address_with_space(
      CreateContractAddressType::FromSenderNonceAndCodeHash,
      &genesis_account,
      &U256::from(index - 1),
      transaction.data(),
    );

    state.set_admin(&contract_address.address, &Address::zero())?;
    state.commit_cache(false);
  }

  state.genesis_special_remove_account(&genesis_account.address)?;

  let prepared_state_root = state.compute_state_root_for_genesis(None)?;

  let transaction_hashes = transactions
    .iter()
    .map(|transaction| transaction.hash())
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  let empty_block_receipts_root = indexed_mpt_root(std::iter::empty::<&[u8]>());
  let receipts_root = indexed_mpt_root(std::iter::once(empty_block_receipts_root.as_bytes()));

  let mut block = Block::new(
    BlockHeaderBuilder::new()
      .with_deferred_state_root(prepared_state_root.aux_info.state_root_hash)
      .with_deferred_receipts_root(receipts_root)
      .with_gas_limit(GENESIS_GAS_LIMIT.into())
      .with_author(header.author)
      .with_difficulty(header.difficulty)
      .with_transactions_root(transactions_root)
      .build(),
    transactions,
  );

  let block_hash = block.block_header.compute_hash();
  let commit_result = state.commit(block_hash, None)?;

  assert_eq!(
    commit_result.state_root, prepared_state_root,
    "Genesis commit changed the prepared state root",
  );

  let committed_state = committed_state_receiver
    .committed_state()
    .expect("successful Genesis commit must hand off its state version");

  assert_eq!(
    committed_state.epoch_id, block_hash,
    "committed Genesis state must use the block hash as epoch identity",
  );
  assert_eq!(
    committed_state.version.root_with_aux_info(),
    commit_result.state_root,
    "handed-off Genesis state must match the committed state root",
  );

  // This cache is populated only after the committed block hash is fixed.
  block.block_header.pow_hash = Some(Default::default());

  Ok(ExecutedGenesis {
    block,
    state_root: commit_result.state_root,
    committed_state,
  })
}

fn build_genesis_transactions(
  chain_id: u32,
  two_year_unlock_token_count: U256,
  four_year_unlock_token_count: U256,
) -> Vec<Arc<SignedTransaction>> {
  let genesis_account = GENESIS_ACCOUNT_ADDRESS.with_native_space();

  let marker = NativeTransaction {
    data: GENESIS_TRANSACTION_DATA_STR.as_bytes().to_vec(),
    action: Action::Call(Address::zero()),
    chain_id,
    ..Default::default()
  };

  let create2_factory = creation_transaction(
    0,
    decode_genesis_bytecode("CREATE2FACTORY", GENESIS_TRANSACTION_CREATE_CREATE2FACTORY),
    U256::zero(),
    300_000,
    512,
    chain_id,
  );

  let two_year_unlock = creation_transaction(
    1,
    decode_genesis_bytecode(
      "TWO_YEAR_UNLOCK",
      GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_TWO_YEAR_UNLOCK,
    ),
    two_year_unlock_token_count,
    2_800_000,
    16_000,
    chain_id,
  );

  let four_year_unlock = creation_transaction(
    2,
    decode_genesis_bytecode(
      "FOUR_YEAR_UNLOCK",
      GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_FOUR_YEAR_UNLOCK,
    ),
    four_year_unlock_token_count,
    5_000_000,
    32_000,
    chain_id,
  );

  let fund_pool_bytecode =
    decode_genesis_bytecode("FUND_POOL", GENESIS_TRANSACTION_CREATE_FUND_POOL);

  let mut transactions = Vec::with_capacity(8);
  transactions.push(Arc::new(marker.fake_sign(Default::default())));
  transactions.push(Arc::new(create2_factory.fake_sign(genesis_account)));
  transactions.push(Arc::new(two_year_unlock.fake_sign(genesis_account)));
  transactions.push(Arc::new(four_year_unlock.fake_sign(genesis_account)));

  for nonce in 3..=6 {
    transactions.push(Arc::new(
      creation_transaction(
        nonce,
        fund_pool_bytecode.clone(),
        U256::zero(),
        400_000,
        1_000,
        chain_id,
      )
      .fake_sign(genesis_account),
    ));
  }

  transactions
}

fn creation_transaction(
  nonce: u64,
  data: Vec<u8>,
  value: U256,
  gas: u64,
  storage_limit: u64,
  chain_id: u32,
) -> NativeTransaction {
  NativeTransaction {
    nonce: nonce.into(),
    gas_price: 1.into(),
    gas: gas.into(),
    action: Action::Create,
    value,
    storage_limit,
    chain_id,
    data,
    ..Default::default()
  }
}

fn decode_genesis_bytecode(contract: &'static str, encoded: &str) -> Vec<u8> {
  encoded
    .from_hex()
    .unwrap_or_else(|source| panic!("invalid Conflux Genesis bytecode for {contract}: {source}"))
}

fn execute_genesis_deployment(
  index: usize,
  contract: &'static str,
  transaction: &SignedTransaction,
  state: &mut State,
  machine: &Machine,
) -> Result<(), GenesisError> {
  let environment = Env {
    transaction_hash: transaction.hash(),
    ..Default::default()
  };
  let spec = machine.spec(environment.number, environment.epoch_height);

  let outcome = ExecutiveContext::new(state, &environment, machine, &spec)
    .transact(transaction, TransactOptions::default())?;

  state.update_state_post_tx_execution(false);

  match outcome {
    ExecutionOutcome::Finished(_) => Ok(()),
    outcome => Err(GenesisError::DeploymentDidNotFinish {
      index,
      contract,
      outcome,
    }),
  }
}

#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc};

  use cfx_executor::{
    machine::{Machine, VmFactory},
    spec::CommonParams,
    state::State,
  };
  use cfx_internal_common::{ChainIdParamsInner, StateRootWithAuxInfo};
  use cfx_parameters::{
    genesis::{
      DEV_GENESIS_KEY_PAIR, DEV_GENESIS_KEY_PAIR_2, GENESIS_ACCOUNT_ADDRESS,
      genesis_contract_address_two_year,
    },
    internal_contract_addresses::ADMIN_CONTROL_CONTRACT_ADDRESS,
  };
  use cfx_statedb::StateDb;
  use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, AllChainID, H256, U256};
  use hex_literal::hex;

  use super::{GenesisHeaderInput, execute_genesis};
  use crate::state::{layered_mpt_state::LayeredMptState, state_version::StateCandidate};

  fn oracle_machine() -> Arc<Machine> {
    let mut params = CommonParams::default();
    params.chain_id = ChainIdParamsInner::new_simple(AllChainID::new(10, 10));

    Arc::new(Machine::new_with_builtin(params, VmFactory::new(32 * 1024)))
  }

  fn oracle_allocations() -> BTreeMap<AddressWithSpace, U256> {
    let balance = U256::from_dec_str("5000000000000000000000000000000000").unwrap();

    BTreeMap::from([
      (DEV_GENESIS_KEY_PAIR.address().with_native_space(), balance),
      (
        DEV_GENESIS_KEY_PAIR_2.address().with_native_space(),
        balance,
      ),
      (DEV_GENESIS_KEY_PAIR.evm_address().with_evm_space(), balance),
      (
        DEV_GENESIS_KEY_PAIR_2.evm_address().with_evm_space(),
        balance,
      ),
    ])
  }

  #[test]
  fn matches_conflux_genesis_oracle() {
    let genesis = execute_genesis(
      oracle_machine(),
      oracle_allocations(),
      GenesisHeaderInput {
        author: GENESIS_ACCOUNT_ADDRESS,
        difficulty: U256::zero(),
      },
    )
    .unwrap();

    let block_hash = H256(hex!(
      "ca6fce203cc4ae51a008c15367170e036792ab1e007b1265159710526954c944"
    ));
    let state_root = StateRootWithAuxInfo::genesis(&H256(hex!(
      "58d1e6734e0b05be59871ccd92ad887f0d5a6ea19da7c6a4969a87108394e511"
    )));

    let header = &genesis.block.block_header;
    assert_eq!(genesis.block.hash(), block_hash);
    assert_eq!(
      (
        genesis.state_root.clone(),
        *header.transactions_root(),
        *header.deferred_receipts_root(),
        *header.deferred_state_root(),
      ),
      (
        state_root.clone(),
        H256(hex!(
          "8208dfdbb409f7a3e41386a8eaaa6412ad4df158fc04a09b499c1a004b53d469"
        )),
        H256(hex!(
          "09f8709ea9f344a810811a373b30861568f5686e649d6177fd92ea2db7477508"
        )),
        state_root.aux_info.state_root_hash,
      )
    );
    assert_eq!(
      (
        genesis.committed_state.epoch_id,
        genesis.committed_state.version.root_with_aux_info(),
      ),
      (block_hash, state_root)
    );

    let (backend, _) = LayeredMptState::new(StateCandidate::new(Arc::clone(
      &genesis.committed_state.version,
    )));
    let state = State::new(StateDb::new(Box::new(backend))).unwrap();
    let allocation_balance = U256::from_dec_str("5000000000000000000000000000000000").unwrap();

    assert_eq!(
      (
        state
          .balance(&DEV_GENESIS_KEY_PAIR.address().with_native_space())
          .unwrap(),
        state
          .balance(&DEV_GENESIS_KEY_PAIR.evm_address().with_evm_space())
          .unwrap(),
      ),
      (allocation_balance, allocation_balance)
    );

    let two_year = genesis_contract_address_two_year();
    assert_eq!(
      (
        state.balance(&two_year).unwrap(),
        state.code_hash(&two_year).unwrap(),
        state.admin(&two_year.address).unwrap(),
      ),
      (
        U256::from_dec_str("799999975000000000000000000").unwrap(),
        H256(hex!(
          "98e7eb536c90167a919e69073b522e94ee174621d9085280ea686c96fe06a36e"
        )),
        Address::zero(),
      )
    );
    assert!(
      state
        .exists(&ADMIN_CONTROL_CONTRACT_ADDRESS.with_native_space())
        .unwrap()
    );

    let genesis_account = GENESIS_ACCOUNT_ADDRESS.with_native_space();
    assert_eq!(
      (
        state.exists(&genesis_account).unwrap(),
        state.balance(&genesis_account).unwrap(),
        state.nonce(&genesis_account).unwrap(),
        state.code_hash(&genesis_account).unwrap(),
      ),
      (
        true,
        U256::zero(),
        U256::zero(),
        H256(hex!(
          "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        )),
      )
    );
  }
}
