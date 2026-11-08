//! Differential checks against Conflux's default State over the same MPT.
use super::mpt_state::{MptStateCandidate, MptStateVersion};
use crate::chain_spec::{ChainIds, ChainSpec};
use cfx_executor::{
  execution_state::ExecutionState,
  executive::{ExecutionOutcome, ExecutiveContext, TransactOptions, TransactSettings},
  internal_contract::initialize_internal_contract_accounts,
  state::State as ReferenceState,
  substate::Substate,
};
use cfx_internal_common::StateRootWithAuxInfo;
use cfx_parameters::consensus::ONE_CFX_IN_DRIP;
use cfx_statedb::{StateDb, StateStorage};
use cfx_storage_types::{Error, MptKeyValue, Result as StorageResult, StateTrait};
use cfx_types::{Address, AddressSpaceUtil, H256, U256};
use cfx_vm_types::Env;
use primitives::{
  Action, EpochId, StorageKeyWithSpace, storage::STORAGE_LAYOUT_REGULAR_V0,
  transaction::eth_transaction::Eip155Transaction,
};
use std::sync::{
  Arc, Mutex,
  atomic::{AtomicBool, Ordering},
};
use xcfx_state::{
  State,
  database::{Database, MptDatabase},
};

/// The default State owns a full-storage object. This test adapter exposes a
/// product MPT through that existing contract and captures its committed version.
struct ReferenceStorage {
  candidate: Option<MptStateCandidate>,
  computed: Option<Arc<MptStateVersion>>,
  committed: Arc<Mutex<Option<Arc<MptStateVersion>>>>,
  fail_after_writes: Option<usize>,
  write_failure_reached: Arc<AtomicBool>,
}

impl ReferenceStorage {
  fn candidate(&self) -> &MptStateCandidate {
    self.candidate.as_ref().expect("test candidate is retained")
  }
  fn candidate_mut(&mut self) -> StorageResult<&mut MptStateCandidate> {
    if let Some(remaining) = &mut self.fail_after_writes {
      if *remaining == 0 {
        self.write_failure_reached.store(true, Ordering::SeqCst);
        return Err(Error::Msg("injected backing write failure".into()));
      }
      *remaining -= 1;
    }
    self.computed = None;
    Ok(self.candidate.as_mut().expect("test candidate is retained"))
  }
}
impl StateTrait for ReferenceStorage {
  fn get(&self, key: StorageKeyWithSpace) -> StorageResult<Option<Box<[u8]>>> {
    StateStorage::get(self.candidate(), key)
  }
  fn set(&mut self, key: StorageKeyWithSpace, value: Box<[u8]>) -> StorageResult<()> {
    StateStorage::set(self.candidate_mut()?, key, value)
  }
  fn delete(&mut self, key: StorageKeyWithSpace) -> StorageResult<()> {
    StateStorage::delete(self.candidate_mut()?, key)
  }
  fn delete_test_only(&mut self, key: StorageKeyWithSpace) -> StorageResult<Option<Box<[u8]>>> {
    StateStorage::delete_test_only(self.candidate_mut()?, key)
  }
  fn delete_all(&mut self, key: StorageKeyWithSpace) -> StorageResult<Option<Vec<MptKeyValue>>> {
    StateStorage::delete_all(self.candidate_mut()?, key)
  }
  fn read_all(&mut self, key: StorageKeyWithSpace) -> StorageResult<Option<Vec<MptKeyValue>>> {
    StateStorage::read_all(
      self.candidate.as_mut().expect("test candidate is retained"),
      key,
    )
  }
  fn read_all_with_callback(
    &mut self,
    key: StorageKeyWithSpace,
    visitor: &mut dyn FnMut(MptKeyValue),
    only_account_key: bool,
  ) -> StorageResult<()> {
    StateStorage::read_all_with_callback(
      self.candidate.as_mut().expect("test candidate is retained"),
      key,
      visitor,
      only_account_key,
    )
  }

  fn compute_state_root(&mut self) -> StorageResult<StateRootWithAuxInfo> {
    let version = Arc::new(
      self
        .candidate
        .take()
        .expect("test candidate is retained")
        .into_version(),
    );
    let root = version.root_with_aux_info();
    self.candidate = Some(MptStateCandidate::new(Arc::clone(&version)));
    self.computed = Some(version);
    Ok(root)
  }
  fn get_state_root(&self) -> StorageResult<StateRootWithAuxInfo> {
    self
      .computed
      .as_ref()
      .map(|version| version.root_with_aux_info())
      .ok_or_else(|| Error::Msg("test root has not been computed".into()))
  }
  fn commit(&mut self, _epoch: EpochId) -> StorageResult<StateRootWithAuxInfo> {
    let root = self.get_state_root()?;
    *self.committed.lock().unwrap() = self.computed.clone();
    Ok(root)
  }
}

pub(crate) fn reference(
  parent: &Arc<MptStateVersion>,
) -> (ReferenceState, Arc<Mutex<Option<Arc<MptStateVersion>>>>) {
  let committed = Arc::new(Mutex::new(None));
  let storage = ReferenceStorage {
    candidate: Some(MptStateCandidate::new(Arc::clone(parent))),
    computed: None,
    committed: Arc::clone(&committed),
    fail_after_writes: None,
    write_failure_reached: Arc::new(AtomicBool::new(false)),
  };
  (
    ReferenceState::new(StateDb::new(Box::new(storage))).unwrap(),
    committed,
  )
}

// Global parameters use internal-contract storage, whose layouts must exist
// before execution state can be committed.
pub(crate) fn initialized_parent(chain: &ChainSpec) -> Arc<MptStateVersion> {
  let empty = Arc::new(MptStateVersion::genesis_parent());
  let (mut state, committed) = reference(&empty);
  initialize_internal_contract_accounts(
    &mut state,
    chain
      .machine()
      .internal_contracts()
      .initialized_at_genesis(),
  )
  .expect("initialize internal-contract accounts");
  state
    .commit(EpochId::default(), None)
    .expect("commit initialized state");
  let mut committed = committed.lock().unwrap();
  committed
    .take()
    .expect("reference storage captures the committed state")
}

fn native_sequence(state: &mut dyn ExecutionState, spec: &cfx_vm_types::Spec) {
  let payer = Address::from([0x10; 20]);
  let other = Address::from([0x11; 20]);
  let contract = Address::from([0x80; 20]).with_native_space();
  state
    .add_balance(&payer.with_native_space(), &U256::from(ONE_CFX_IN_DRIP))
    .unwrap();
  state
    .new_contract_with_admin(
      &contract,
      &payer,
      U256::zero(),
      Some(STORAGE_LAYOUT_REGULAR_V0),
      true,
    )
    .unwrap();
  state.commit_cache(false);
  state.checkpoint();
  let mut retained = Substate::new();
  state
    .set_storage(&contract, vec![0; 32], U256::from(7), payer, &mut retained)
    .unwrap();
  state.checkpoint();
  let mut discarded = Substate::new();
  state
    .set_storage(&contract, vec![0; 32], U256::from(9), other, &mut discarded)
    .unwrap();
  state.revert_to_checkpoint();
  assert_eq!(
    state.storage_at(&contract, &[0; 32]).unwrap(),
    U256::from(7)
  );
  state
    .settle_collateral_for_all(&retained, &mut (), spec, false)
    .unwrap()
    .unwrap();
  state.discard_checkpoint();
  let collateral = U256::from(ONE_CFX_IN_DRIP / 16);
  assert_eq!(state.collateral_for_storage(&payer).unwrap(), collateral);
  assert_eq!(
    state.balance(&payer.with_native_space()).unwrap(),
    U256::from(ONE_CFX_IN_DRIP) - collateral
  );
  state.commit_cache(false);
}

#[test]
fn native_collateral_and_checkpoint_match_default_state() {
  let chain = ChainSpec::new(ChainIds::default());
  let parent = initialized_parent(&chain);
  let spec = chain.machine().spec(1, 1);
  let (mut expected, committed) = reference(&parent);
  expected.add_total_issued(U256::from(ONE_CFX_IN_DRIP));
  native_sequence(&mut expected, &spec);
  let expected_root = expected
    .commit(H256::from_low_u64_be(1), None)
    .unwrap()
    .state_root;
  let mut candidate = MptStateCandidate::new(Arc::clone(&parent));
  let mut actual = State::from_database(Box::new(MptDatabase::new(&mut candidate))).unwrap();
  actual.add_total_issued(U256::from(ONE_CFX_IN_DRIP));
  native_sequence(&mut actual, &spec);
  actual.apply_changes_to_storage(None).unwrap();
  drop(actual);
  let actual = Arc::new(candidate.into_version());
  assert_eq!(actual.root_with_aux_info(), expected_root);
  assert_eq!(
    committed
      .lock()
      .unwrap()
      .as_ref()
      .unwrap()
      .root_with_aux_info(),
    expected_root
  );
  let account = actual
    .open_database()
    .get_account(&Address::from([0x10; 20]).with_native_space())
    .unwrap()
    .unwrap();
  assert_eq!(
    account.collateral_for_storage,
    U256::from(ONE_CFX_IN_DRIP / 16)
  );
}

fn execute_espace_store(state: &mut dyn ExecutionState, chain: &ChainSpec) -> U256 {
  let sender = Address::from_low_u64_be(0x100).with_space(cfx_types::Space::Ethereum);
  let contract = Address::from_low_u64_be(0x200).with_space(cfx_types::Space::Ethereum);
  state
    .add_balance(&sender, &U256::from(ONE_CFX_IN_DRIP))
    .unwrap();
  state.add_total_evm_tokens(U256::from(ONE_CFX_IN_DRIP));
  state
    .new_contract_with_admin(
      &contract,
      &Address::zero(),
      U256::zero(),
      Some(STORAGE_LAYOUT_REGULAR_V0),
      false,
    )
    .unwrap();
  state
    .init_code(
      &contract,
      vec![0x60, 0x2a, 0x60, 0x00, 0x55, 0x00],
      Address::zero(),
      H256::zero(),
    )
    .unwrap();
  state.commit_cache(false);
  let tx = Eip155Transaction {
    gas: U256::from(100_000),
    action: Action::Call(contract.address),
    chain_id: Some(ChainIds::default().espace_chain_id),
    ..Default::default()
  }
  .fake_sign_rpc(sender);
  let env = Env {
    number: 1,
    epoch_height: 1,
    chain_id: chain.machine().params().chain_id_map(1),
    gas_limit: U256::from(1_000_000),
    transaction_hash: tx.hash(),
    ..Default::default()
  };
  let spec = chain.machine().spec(1, 1);
  let result = ExecutiveContext::new(state, &env, chain.machine(), &spec)
    .transact(
      &tx,
      TransactOptions {
        observer: (),
        settings: TransactSettings {
          charge_gas: false,
          check_base_price: false,
          check_epoch_bound: false,
          ..TransactSettings::all_checks()
        },
      },
    )
    .unwrap();
  let ExecutionOutcome::Finished(executed) = result else {
    panic!("store transaction did not finish: {result:?}")
  };
  assert_eq!(
    state.storage_at(&contract, &[0; 32]).unwrap(),
    U256::from(42)
  );
  assert_eq!(state.nonce(&sender).unwrap(), U256::one());
  executed.gas_used
}

#[test]
fn espace_vm_and_mpt_writeback_match_default_state() {
  let chain = ChainSpec::new(ChainIds::default());
  let parent = initialized_parent(&chain);
  let (mut expected, _) = reference(&parent);
  expected.add_total_issued(U256::from(ONE_CFX_IN_DRIP));
  let expected_gas = execute_espace_store(&mut expected, &chain);
  expected.update_state_post_tx_execution(false);
  let expected_root = expected
    .commit(H256::from_low_u64_be(1), None)
    .unwrap()
    .state_root;
  let mut candidate = MptStateCandidate::new(Arc::clone(&parent));
  let mut actual = State::from_database(Box::new(MptDatabase::new(&mut candidate))).unwrap();
  actual.add_total_issued(U256::from(ONE_CFX_IN_DRIP));
  let actual_gas = execute_espace_store(&mut actual, &chain);
  actual.update_state_post_tx_execution(false);
  actual.apply_changes_to_storage(None).unwrap();
  drop(actual);
  assert_eq!(actual_gas, expected_gas);
  assert_eq!(candidate.into_version().root_with_aux_info(), expected_root);
}

#[test]
fn failed_writeback_leaves_parent_and_sibling_versions_unchanged() {
  let chain = ChainSpec::new(ChainIds::default());
  let parent = initialized_parent(&chain);
  let before = parent.root_with_aux_info();
  let sibling = MptStateCandidate::new(Arc::clone(&parent));
  let failure = Arc::new(AtomicBool::new(false));
  let committed = Arc::new(Mutex::new(None));
  let storage = ReferenceStorage {
    candidate: Some(MptStateCandidate::new(Arc::clone(&parent))),
    computed: None,
    committed: Arc::clone(&committed),
    fail_after_writes: Some(1),
    write_failure_reached: Arc::clone(&failure),
  };
  let mut state = State::from_database(Box::new(MptDatabase::new(
    Box::new(storage) as Box<dyn StateTrait>
  )))
  .unwrap();
  state.add_total_issued(U256::from(ONE_CFX_IN_DRIP));
  native_sequence(&mut state, &chain.machine().spec(1, 1));
  let error = state
    .apply_changes_to_storage(None)
    .expect_err("the injected write failure must abort writeback");
  drop(state);
  assert!(
    failure.load(Ordering::SeqCst),
    "writeback failed before the injected failure: {error:?}"
  );
  assert!(committed.lock().unwrap().is_none());
  assert_eq!(parent.root_with_aux_info(), before);
  assert_eq!(sibling.into_version().root_with_aux_info(), before);
}
