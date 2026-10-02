//! Builds and executes the local Conflux genesis transition.

use std::{collections::BTreeMap, sync::Arc};

use cfx_executor::{
  executive::{ExecutionOutcome, ExecutiveContext, TransactOptions},
  internal_contract::{initialize_internal_contract_accounts, make_staking_events},
  machine::Machine,
  state::{State, initialize_cip107, initialize_cip137, initialize_or_update_dao_voted_params},
};
use cfx_parameters::{
  consensus::{GENESIS_GAS_LIMIT, ONE_CFX_IN_DRIP},
  consensus_internal::{GENESIS_TOKEN_COUNT_IN_CFX, TWO_YEAR_UNLOCK_TOKEN_COUNT_IN_CFX},
  genesis::{
    GENESIS_ACCOUNT_ADDRESS, GENESIS_TRANSACTION_CREATE_CREATE2FACTORY,
    GENESIS_TRANSACTION_CREATE_FUND_POOL,
    GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_FOUR_YEAR_UNLOCK,
    GENESIS_TRANSACTION_CREATE_GENESIS_TOKEN_MANAGER_TWO_YEAR_UNLOCK, GENESIS_TRANSACTION_DATA_STR,
  },
  staking::POS_VOTE_PRICE,
};
use cfx_statedb::StateDb;
use cfx_types::{
  Address, AddressSpaceUtil, AddressWithSpace, CreateContractAddressType, H256, Space, SpaceMap,
  U256, cal_contract_address_with_space,
};
use diem_crypto::ValidCryptoMaterial;
use diem_types::{block_info::PivotBlockDecision, term_state::pos_state_config::PosStateConfig};
use pow_types::StakingEvent;

use cfx_vm_types::Env;
use primitives::{
  Action, Block, BlockHeaderBuilder, BlockReceipts, Cip112TransitionHeight, SignedTransaction,
  transaction::native_transaction::NativeTransaction,
};
use rustc_hex::FromHex;
use thiserror::Error;

use crate::{
  execution::{ExecutionCommitment, compute_epoch_receipts_root},
  mpt::indexed_mpt_root,
  pos::{
    CommittedPosState, GENESIS_POS_REFERENCE, GenesisPosDefinition, bootstrap_genesis_pos_state,
  },
  state::{
    balance::{BalanceChangeError, mint_balance},
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GenesisHeaderInput {
  pub(crate) author: Address,
  pub(crate) difficulty: U256,
  pub(crate) custom: Vec<Vec<u8>>,
  pub(crate) base_price: Option<SpaceMap<U256>>,
}

pub(crate) struct ExecutedGenesis {
  pub(crate) block: Block,
  pub(crate) committed_state: CommittedStateVersion,
  pub(crate) commitment: ExecutionCommitment,
  pub(crate) block_receipts: Vec<Arc<BlockReceipts>>,
}

pub(crate) struct ExecutedGenesisWithPos {
  pub(crate) execution: ExecutedGenesis,
  pub(crate) committed_pos_state: CommittedPosState,
}

#[derive(Debug, Error)]
pub(crate) enum GenesisError {
  #[error("Genesis state access failed")]
  State(#[from] cfx_statedb::Error),

  #[error("Genesis allocations: {0}")]
  Allocation(#[from] BalanceChangeError),

  #[error("Genesis contract deployment transaction {index} ({contract}) did not finish")]
  DeploymentDidNotFinish {
    index: usize,
    contract: &'static str,
    outcome: ExecutionOutcome,
  },
  #[error("Genesis PoS registration transaction {index} did not finish")]
  PosRegistrationDidNotFinish {
    index: usize,
    outcome: ExecutionOutcome,
  },
}

pub(crate) fn execute_genesis(
  machine: Arc<Machine>,
  allocations: BTreeMap<AddressWithSpace, U256>,
  header: GenesisHeaderInput,
) -> Result<ExecutedGenesis, GenesisError> {
  execute_genesis_inner(machine, allocations, header, None)
}

pub(crate) fn execute_genesis_with_pos(
  machine: Arc<Machine>,
  allocations: BTreeMap<AddressWithSpace, U256>,
  header: GenesisHeaderInput,
  definition: &GenesisPosDefinition,
  pos_state_config: &PosStateConfig,
) -> Result<ExecutedGenesisWithPos, GenesisError> {
  let execution = execute_genesis_inner(machine, allocations, header, Some(definition))?;

  let committed_pos_state = bootstrap_genesis_pos_state(
    definition,
    pos_state_config,
    PivotBlockDecision {
      height: 0,
      block_hash: execution.block.hash(),
    },
  );

  Ok(ExecutedGenesisWithPos {
    execution,
    committed_pos_state,
  })
}

fn execute_genesis_inner(
  machine: Arc<Machine>,
  allocations: BTreeMap<AddressWithSpace, U256>,
  header: GenesisHeaderInput,
  pos_definition: Option<&GenesisPosDefinition>,
) -> Result<ExecutedGenesis, GenesisError> {
  let parent = Arc::new(StateVersion::genesis_parent());
  let mut candidate = StateCandidate::new(parent);
  let mut state = State::new(StateDb::new(&mut candidate))?;

  initialize_internal_contract_accounts(
    &mut state,
    machine.internal_contracts().initialized_at_genesis(),
  )?;

  // Ordinary block hooks start after Genesis and cannot apply transitions at block zero.
  let transitions = &machine.params().transition_numbers;
  if transitions.cip94n == 0 {
    // CIP-105 snapshots PoS staking only after its activation block.
    initialize_or_update_dao_voted_params(&mut state, false)?;
  }
  if transitions.cip107 == 0 {
    initialize_cip107(&mut state)?;
  }
  if transitions.cip137 == 0 {
    initialize_cip137(&mut state);
  }

  let genesis_token_count = U256::from(GENESIS_TOKEN_COUNT_IN_CFX) * U256::from(ONE_CFX_IN_DRIP);
  let two_year_unlock_token_count =
    U256::from(TWO_YEAR_UNLOCK_TOKEN_COUNT_IN_CFX) * U256::from(ONE_CFX_IN_DRIP);
  let four_year_unlock_token_count = genesis_token_count - two_year_unlock_token_count;

  // Reserve protocol issuance before applying configurable allocations.
  state.add_total_issued(genesis_token_count);
  for (address, balance) in allocations {
    mint_balance(&mut state, &address, balance)?;
  }

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

  if let Some(definition) = pos_definition {
    execute_genesis_pos(&mut state, machine.as_ref(), definition)?;
  }

  state.genesis_special_remove_account(&genesis_account.address)?;

  state.apply_changes_to_storage(None)?;
  drop(state);

  let version = Arc::new(candidate.into_version());
  let state_root = version
    .root_with_aux_info()
    .expect("local Genesis must produce a complete MPT state");

  let transaction_hashes = transactions
    .iter()
    .map(|transaction| transaction.hash())
    .collect::<Vec<_>>();

  let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

  let block_receipts = vec![Arc::new(BlockReceipts {
    receipts: Vec::new(),
    block_number: 0,
    secondary_reward: U256::zero(),
    tx_execution_error_messages: Vec::new(),
  })];
  let receipts_root = compute_epoch_receipts_root(&block_receipts);
  let logs_bloom_hash = BlockHeaderBuilder::compute_block_logs_bloom_hash(&block_receipts);

  let cip112_transition_height =
    Cip112TransitionHeight::new(machine.params().transition_heights.cip112);

  let mut block = Block::new(
    BlockHeaderBuilder::new()
      .with_deferred_state_root(state_root.aux_info.state_root_hash)
      .with_deferred_receipts_root(receipts_root)
      .with_deferred_logs_bloom_hash(logs_bloom_hash)
      .with_gas_limit(GENESIS_GAS_LIMIT.into())
      .with_author(header.author)
      .with_difficulty(header.difficulty)
      .with_transactions_root(transactions_root)
      .with_custom(header.custom)
      .with_pos_reference(pos_definition.map(|_| GENESIS_POS_REFERENCE))
      .with_base_price(header.base_price)
      .build_with_cip112(cip112_transition_height),
    transactions,
  );

  let block_hash = block.block_header.compute_hash();
  let committed_state = CommittedStateVersion {
    epoch_id: block_hash,
    version,
  };
  let commitment = ExecutionCommitment {
    state_root: Some(state_root.aux_info.state_root_hash),
    receipts_root,
    logs_bloom_hash,
  };

  // This cache is populated only after the committed block hash is fixed.
  block.block_header.pow_hash = Some(Default::default());

  Ok(ExecutedGenesis {
    block,
    committed_state,
    commitment,
    block_receipts,
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
  state: &mut State<'_>,
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

fn execute_genesis_pos(
  state: &mut State<'_>,
  machine: &Machine,
  definition: &GenesisPosDefinition,
) -> Result<(), GenesisError> {
  for (index, node) in definition.initial_nodes.iter().enumerate() {
    let stake_balance = U256::from(node.voting_power) * *POS_VOTE_PRICE;
    let account_balance = stake_balance + U256::from(ONE_CFX_IN_DRIP) * U256::from(20);
    let native_address = node.execution_address.with_native_space();
    state.add_balance(&native_address, &account_balance)?;
    state.deposit(&node.execution_address, &stake_balance, 0, false)?;
    state.commit_cache(false);

    let signed_transaction = node.register_transaction.clone().fake_sign(native_address);

    let environment = Env {
      transaction_hash: signed_transaction.hash(),
      ..Default::default()
    };
    let mut spec = machine.spec(environment.number, environment.epoch_height);
    spec.cip43_init = true;

    let outcome = ExecutiveContext::new(state, &environment, machine, &spec)
      .transact(&signed_transaction, TransactOptions::default())?;

    state.update_state_post_tx_execution(false);

    let executed = match outcome {
      ExecutionOutcome::Finished(executed) => executed,
      outcome => {
        return Err(GenesisError::PosRegistrationDidNotFinish { index, outcome });
      }
    };

    let identifier = H256::from_slice(node.node_id.addr.as_ref());
    let expected_events = vec![
      StakingEvent::Register(
        identifier,
        node.node_id.public_key.to_bytes(),
        node.node_id.vrf_public_key.to_bytes(),
      ),
      StakingEvent::IncreaseStake(identifier, node.voting_power),
    ];

    assert_eq!(
      make_staking_events(&executed.logs),
      expected_events,
      "Genesis PoS registration transaction {index} emitted unexpected events",
    );
  }

  Ok(())
}

#[cfg(test)]
mod tests {
  use std::{collections::BTreeMap, sync::Arc};

  use cfx_executor::{
    internal_contract::{block_hash_slot, epoch_hash_slot},
    machine::Machine,
    state::State,
  };
  use cfx_parameters::{
    consensus::{GENESIS_GAS_LIMIT, NEXT_HARDFORK_HEADER_CUSTOM_FIRST_ELEMENT, ONE_CFX_IN_DRIP},
    consensus_internal::INITIAL_1559_CORE_BASE_PRICE,
    genesis::{
      DEV_GENESIS_KEY_PAIR, DEV_GENESIS_KEY_PAIR_2, GENESIS_ACCOUNT_ADDRESS,
      genesis_contract_address_two_year,
    },
    internal_contract_addresses::{ADMIN_CONTROL_CONTRACT_ADDRESS, POS_REGISTER_CONTRACT_ADDRESS},
    staking::POS_VOTE_PRICE,
  };
  use cfx_types::{Address, AddressSpaceUtil, AddressWithSpace, H256, U256};
  use hex_literal::hex;

  use super::{GenesisHeaderInput, execute_genesis, execute_genesis_with_pos};
  use crate::state::state_version::StateVersion;

  use diem_crypto::ValidCryptoMaterialStringExt;
  use diem_types::{
    block_info::PivotBlockDecision,
    term_state::NodeID,
    validator_config::{ConsensusPublicKey, ConsensusVRFPublicKey},
  };
  use primitives::{
    Action, Block, BlockHeaderBuilder, Cip112TransitionHeight, Receipt, Transaction,
    TransactionStatus, transaction::native_transaction::NativeTransaction,
  };

  use crate::{
    block_producer::RuntimeBlock,
    chain_spec::{ChainIds, ChainSpec},
    mpt::indexed_mpt_root,
    pos::{GenesisPosDefinition, GenesisPosNode, PosEnvInput},
    production_environment::ProductionDefaults,
    runtime::{NodeRuntime, RuntimeConfig},
    transaction_pool::TransactionPoolPolicy,
  };

  const CONFLUX_COMPATIBILITY_CHAIN_ID: u32 = 10;
  const INITIAL_POS_VOTING_POWER: u64 = 2_000;
  const COMPATIBILITY_ALLOCATION_BALANCE: &str = "5000000000000000000000000000000000";

  const SINGLE_VALIDATOR_REGISTER_CALL_DATA: &[u8] = &hex!(
    "e335b451b4e1f5b4fe44955f08099a5441d9a0d8b2837dfc9ce69c6c9c86bedc792e847d00000000"
    "000000000000000000000000000000000000000000000000000007d0000000000000000000000000"
    "00000000000000000000000000000000000000a00000000000000000000000000000000000000000"
    "00000000000000000000010000000000000000000000000000000000000000000000000000000000"
    "000001600000000000000000000000000000000000000000000000000000000000000030b157f238"
    "403a5b980546fd19ca48f79a2613e3e3a91d14ee69908b8816e4c53665370b2fbd0db62cc4aa0e8c"
    "aeedc9b5000000000000000000000000000000000000000000000000000000000000000000000000"
    "0000000000000000000000210250356d2d32863cc527c457721de856665365949e3b4f977a9ff167"
    "131ce638d10000000000000000000000000000000000000000000000000000000000000000000000"
    "00000000000000000000000000000000000000000000000000000040000000000000000000000000"
    "00000000000000000000000000000000000000a00000000000000000000000000000000000000000"
    "000000000000000000000030927de16a6b4f669b4c17088271001faf51799496f3d491cb21ce9f5c"
    "d910b0396b1e520830c15df149438c8eb7e3ddbe0000000000000000000000000000000000000000"
    "00000000000000000000000000000000000000000000000000000020f4d418316b1d4bc0d2e2e2b7"
    "192a7a80998f5a83ffb53a6b48ec219f08511939"
  );

  fn single_validator_genesis_definition() -> GenesisPosDefinition {
    let bls_key = ConsensusPublicKey::from_encoded_string(
      "b157f238403a5b980546fd19ca48f79a2613e3e3a91d14ee69908b8816e4c53665370b2fbd0db62cc4aa0e8caeedc9b5",
    )
    .unwrap();

    let vrf_key = ConsensusVRFPublicKey::from_encoded_string(
      "0250356d2d32863cc527c457721de856665365949e3b4f977a9ff167131ce638d1",
    )
    .unwrap();

    let node_id = NodeID::new(bls_key, vrf_key);
    let voting_power = INITIAL_POS_VOTING_POWER;

    GenesisPosDefinition {
      initial_seed: H256(hex!(
        "0909090909090909090909090909090909090909090909090909090909090909"
      )),
      initial_nodes: vec![GenesisPosNode {
        execution_address: DEV_GENESIS_KEY_PAIR_2.address(),
        node_id: node_id.clone(),
        voting_power,
        register_transaction: NativeTransaction {
          nonce: 0.into(),
          gas_price: 1.into(),
          gas: 200_000.into(),
          action: Action::Call(POS_REGISTER_CONTRACT_ADDRESS),
          value: U256::zero(),
          storage_limit: 16_000,
          chain_id: CONFLUX_COMPATIBILITY_CHAIN_ID,
          data: SINGLE_VALIDATOR_REGISTER_CALL_DATA.to_vec(),
          ..Default::default()
        },
      }],
      initial_committee: vec![(node_id.addr, voting_power)],
    }
  }

  fn conflux_compatibility_protocol() -> (Arc<ChainSpec>, GenesisHeaderInput) {
    let chain_spec = Arc::new(ChainSpec::new(ChainIds {
      chain_id: CONFLUX_COMPATIBILITY_CHAIN_ID,
      espace_chain_id: CONFLUX_COMPATIBILITY_CHAIN_ID,
      network_id: u64::from(CONFLUX_COMPATIBILITY_CHAIN_ID),
    }));
    let params = chain_spec.machine().params();

    let custom = params
      .custom_prefix(0)
      .expect("the configured protocol rules must define Genesis custom data");

    assert_eq!(
      custom,
      vec![NEXT_HARDFORK_HEADER_CUSTOM_FIRST_ELEMENT.to_vec()],
    );

    let header = GenesisHeaderInput {
      author: GENESIS_ACCOUNT_ADDRESS,
      difficulty: U256::zero(),
      custom,
      base_price: Some(params.init_base_price()),
    };

    (chain_spec, header)
  }

  fn conflux_compatibility_allocations() -> BTreeMap<AddressWithSpace, U256> {
    let balance = U256::from_dec_str(COMPATIBILITY_ALLOCATION_BALANCE).unwrap();

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

  fn first_core_transfer_block(machine: &Machine, genesis_block: &Block) -> Block {
    let transaction = Arc::new(
      Transaction::from(NativeTransaction {
        nonce: U256::zero(),
        gas_price: U256::from(INITIAL_1559_CORE_BASE_PRICE),
        gas: U256::from(21_000),
        action: Action::Call(DEV_GENESIS_KEY_PAIR_2.address()),
        value: U256::from(ONE_CFX_IN_DRIP),
        storage_limit: 0,
        epoch_height: 1,
        chain_id: CONFLUX_COMPATIBILITY_CHAIN_ID,
        data: Vec::new(),
      })
      .sign(DEV_GENESIS_KEY_PAIR.secret()),
    );

    let transactions = vec![transaction];
    let transaction_hashes = transactions
      .iter()
      .map(|transaction| transaction.hash())
      .collect::<Vec<_>>();
    let transactions_root = indexed_mpt_root(transaction_hashes.iter().map(|hash| hash.as_bytes()));

    let parent_header = &genesis_block.block_header;
    let custom = machine
      .params()
      .custom_prefix(1)
      .expect("the configured protocol rules must define first-block custom data");
    let cip112 = Cip112TransitionHeight::new(machine.params().transition_heights.cip112);

    Block::new(
      BlockHeaderBuilder::new()
        .with_parent_hash(genesis_block.hash())
        .with_height(1)
        .with_timestamp(1)
        .with_author(DEV_GENESIS_KEY_PAIR.address())
        .with_transactions_root(transactions_root)
        .with_deferred_state_root(*parent_header.deferred_state_root())
        .with_deferred_receipts_root(*parent_header.deferred_receipts_root())
        .with_deferred_logs_bloom_hash(*parent_header.deferred_logs_bloom_hash())
        .with_difficulty(U256::one())
        .with_gas_limit(U256::from(GENESIS_GAS_LIMIT))
        .with_custom(custom)
        .with_pos_reference(parent_header.pos_reference().to_owned())
        .with_base_price(Some(machine.params().min_base_price()))
        .build_with_cip112(cip112),
      transactions,
    )
  }
  fn open_committed_state(version: &Arc<StateVersion>) -> State<'static> {
    State::new(version.open_database()).expect("a committed Genesis state must be readable")
  }

  #[test]
  fn conflux_genesis_initializes_state_and_contracts() {
    let (chain_spec, header) = conflux_compatibility_protocol();

    let genesis = execute_genesis(
      Arc::clone(chain_spec.machine()),
      conflux_compatibility_allocations(),
      header,
    )
    .expect("the fixed Conflux compatibility Genesis must execute");

    let state_root = genesis
      .committed_state
      .version
      .root_with_aux_info()
      .expect("Genesis has an MPT root");

    let header = &genesis.block.block_header;
    assert_eq!(
      genesis.commitment.state_root,
      Some(state_root.aux_info.state_root_hash),
      "Genesis commitment must describe the committed state",
    );
    assert_eq!(
      *header.transactions_root(),
      H256(hex!(
        "8208dfdbb409f7a3e41386a8eaaa6412ad4df158fc04a09b499c1a004b53d469"
      )),
      "Genesis transaction root changed",
    );
    assert_eq!(
      *header.deferred_receipts_root(),
      H256(hex!(
        "09f8709ea9f344a810811a373b30861568f5686e649d6177fd92ea2db7477508"
      )),
      "Genesis receipt root changed",
    );
    assert_eq!(
      *header.deferred_state_root(),
      state_root.aux_info.state_root_hash,
      "Genesis header must commit its initial state",
    );

    // The committed version must describe the same block and state.
    assert_eq!(
      genesis.committed_state.epoch_id,
      genesis.block.hash(),
      "committed Genesis state must use the Genesis block identity",
    );

    // Read representative state through the committed-state boundary.
    let state = open_committed_state(&genesis.committed_state.version);
    let allocation_balance = U256::from_dec_str(COMPATIBILITY_ALLOCATION_BALANCE).unwrap();

    assert_eq!(state.pow_base_reward(), U256::from(2 * ONE_CFX_IN_DRIP));
    assert_eq!(
      state.storage_point_prop().unwrap(),
      U256::from(ONE_CFX_IN_DRIP)
    );
    assert_eq!(state.get_base_price_prop(), U256::from(ONE_CFX_IN_DRIP));

    assert_eq!(
      state
        .balance(&DEV_GENESIS_KEY_PAIR.address().with_native_space())
        .unwrap(),
      allocation_balance,
      "Core allocation changed",
    );
    assert_eq!(
      state
        .balance(&DEV_GENESIS_KEY_PAIR.evm_address().with_evm_space())
        .unwrap(),
      allocation_balance,
      "eSpace allocation changed",
    );

    let two_year = genesis_contract_address_two_year();
    assert_eq!(
      state.balance(&two_year).unwrap(),
      U256::from_dec_str("799999975000000000000000000").unwrap(),
      "two-year unlock balance changed",
    );
    assert_eq!(
      state.code_hash(&two_year).unwrap(),
      H256(hex!(
        "98e7eb536c90167a919e69073b522e94ee174621d9085280ea686c96fe06a36e"
      )),
      "two-year unlock bytecode changed",
    );
    assert_eq!(
      state.admin(&two_year.address).unwrap(),
      Address::zero(),
      "two-year unlock admin changed",
    );
    assert!(
      state
        .exists(&ADMIN_CONTROL_CONTRACT_ADDRESS.with_native_space())
        .unwrap(),
      "AdminControl must exist after Genesis",
    );

    let genesis_account = GENESIS_ACCOUNT_ADDRESS.with_native_space();
    assert!(
      state.exists(&genesis_account).unwrap(),
      "Genesis sentinel account must remain observable",
    );
    assert_eq!(
      state.balance(&genesis_account).unwrap(),
      U256::zero(),
      "Genesis sentinel account balance changed",
    );
    assert_eq!(
      state.nonce(&genesis_account).unwrap(),
      U256::zero(),
      "Genesis sentinel account nonce changed",
    );
    assert_eq!(
      state.code_hash(&genesis_account).unwrap(),
      H256(hex!(
        "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
      )),
      "Genesis sentinel account code hash changed",
    );
  }

  #[test]
  fn genesis_commits_initial_pos_state() {
    let definition = single_validator_genesis_definition();
    let node = &definition.initial_nodes[0];
    let expected_staking = U256::from(node.voting_power) * *POS_VOTE_PRICE;

    let (chain_spec, header) = conflux_compatibility_protocol();

    let genesis = execute_genesis_with_pos(
      Arc::clone(chain_spec.machine()),
      conflux_compatibility_allocations(),
      header,
      &definition,
      chain_spec.pos_state_config(),
    )
    .expect("the fixed initial PoS registration must execute");

    let pos_env_input = genesis
      .execution
      .block
      .block_header
      .pos_reference()
      .as_ref()
      .and_then(|reference| genesis.committed_pos_state.env_input(reference));
    let genesis_hash = genesis.execution.block.hash();
    assert_eq!(
      genesis.execution.committed_state.epoch_id, genesis_hash,
      "committed execution state must use the PoS Genesis block identity",
    );
    assert_eq!(
      genesis.committed_pos_state.pivot_decision(),
      &PivotBlockDecision {
        height: 0,
        block_hash: genesis_hash,
      },
      "committed PoS state must pivot on the PoS Genesis block",
    );
    assert_eq!(
      pos_env_input,
      Some(PosEnvInput {
        pos_view: 1,
        finalized_epoch: 0,
      }),
      "Genesis PoS reference must provide the first block's PoS environment",
    );

    let state = open_committed_state(&genesis.execution.committed_state.version);
    assert_eq!(
      state.staking_balance(&node.execution_address).unwrap(),
      expected_staking,
      "initial PoS deposit was not committed",
    );
    assert_eq!(
      state.pos_locked_staking(&node.execution_address).unwrap(),
      expected_staking,
      "initial PoS registration was not committed",
    );
    assert_eq!(
      state.total_pos_staking_tokens(),
      expected_staking,
      "global PoS staking total was not updated",
    );
  }

  #[test]
  fn ordered_core_transfer_commits_balances_fees_and_history() {
    let definition = single_validator_genesis_definition();
    let (chain_spec, header) = conflux_compatibility_protocol();

    let mut runtime = NodeRuntime::from_genesis(
      RuntimeConfig {
        chain_spec: Arc::clone(&chain_spec),
        production_defaults: ProductionDefaults::new(1),
        signing_keys: Default::default(),
        transaction_pool_policy: TransactionPoolPolicy::new(1_024),
        max_state_controls: 0,
      },
      conflux_compatibility_allocations(),
      header,
      definition,
    )
    .expect("the fixed PoS Genesis must execute");

    let sender = DEV_GENESIS_KEY_PAIR.address().with_native_space();
    let receiver = DEV_GENESIS_KEY_PAIR_2.address().with_native_space();

    let parent_view = runtime.optimistic_head();
    let parent_state = open_committed_state(&parent_view.state().version);
    let parent_sender_balance = parent_state.balance(&sender).unwrap();
    let parent_receiver_balance = parent_state.balance(&receiver).unwrap();
    let parent_total_issued = parent_state.total_issued_tokens();

    let parent_artifacts = parent_view
      .artifacts()
      .expect("Local Genesis must retain its execution artifacts");
    let genesis_hash = parent_artifacts.pivot_block().hash();
    let block = first_core_transfer_block(chain_spec.machine(), parent_artifacts.pivot_block());
    let commit_outcome = runtime
      .execute_and_commit_single_block_epoch(RuntimeBlock::from_recovered_block(block))
      .expect("the fixed Core transfer block must execute");
    let executed_view = runtime.optimistic_head();
    let artifacts = executed_view
      .artifacts()
      .expect("local block execution must retain its epoch artifacts");

    let receipts = &artifacts.block_receipts()[0];
    let transaction_fee = U256::from(21_000) * U256::from(INITIAL_1559_CORE_BASE_PRICE);
    assert_eq!(
      receipts.receipts,
      vec![Receipt {
        accumulated_gas_used: U256::from(21_000),
        gas_fee: transaction_fee,
        outcome_status: TransactionStatus::Success,
        burnt_gas_fee: Some(transaction_fee / U256::from(2)),
        ..Default::default()
      }],
    );
    assert!(receipts.tx_execution_error_messages[0].is_empty());
    assert!(
      commit_outcome
        .transaction_pool_updates
        .transactions_to_repack
        .is_empty(),
    );

    let executed_block = artifacts.pivot_block();
    assert_eq!(*executed_block.block_header.parent_hash(), genesis_hash);
    assert_eq!(
      executed_block.transactions[0].hash(),
      H256(hex!(
        "3957d7bdaaee6ff2660ad54681c5f4f0c1fb794c99cd95621b2de7a18e0dfa65"
      )),
    );

    let commitment = artifacts.commitment();
    assert_eq!(
      commitment.logs_bloom_hash,
      H256(hex!(
        "d397b3b043d87fcd6fad1291ff0bfd16401c274896d8c63a923727f077b8e0b5"
      )),
    );
    assert_eq!(
      commitment.state_root,
      Some(
        executed_view
          .state()
          .version
          .root_with_aux_info()
          .expect("local execution has an MPT root")
          .aux_info
          .state_root_hash,
      ),
    );

    let executed_state = open_committed_state(&executed_view.state().version);
    assert_eq!(executed_state.nonce(&sender).unwrap(), U256::one());
    assert_eq!(
      executed_state.balance(&sender).unwrap(),
      parent_sender_balance - U256::from(ONE_CFX_IN_DRIP) - transaction_fee,
    );
    assert_eq!(
      executed_state.balance(&receiver).unwrap(),
      parent_receiver_balance + U256::from(ONE_CFX_IN_DRIP),
    );
    assert_eq!(
      executed_state.total_issued_tokens(),
      parent_total_issued - transaction_fee / U256::from(2),
      "CIP-137 must burn half of the base fee with the initial share ratio",
    );

    for slot in [block_hash_slot(0), epoch_hash_slot(0)] {
      assert_eq!(
        executed_state.get_system_storage(&slot).unwrap(),
        U256::from_big_endian(genesis_hash.as_bytes()),
        "the first local epoch must make the Genesis hash available",
      );
      assert_eq!(
        parent_state.get_system_storage(&slot).unwrap(),
        U256::zero()
      );
    }

    assert_eq!(parent_state.nonce(&sender).unwrap(), U256::zero());
    assert_eq!(
      parent_state.balance(&sender).unwrap(),
      parent_sender_balance
    );
    assert_eq!(parent_state.total_issued_tokens(), parent_total_issued);
    assert_eq!(
      parent_state.balance(&receiver).unwrap(),
      parent_receiver_balance,
    );
  }
}
