use super::*;
use crate::{
  chain_spec::{ChainIds, ChainSpec},
  fork::tests::MockFork,
};
use cfx_executor::{
  execution_state::ExecutionState, internal_contract::storage_point_prop, substate::Substate,
};
use cfx_parameters::{
  consensus::ONE_CFX_IN_DRIP,
  internal_contract_addresses::{
    SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS, SYSTEM_STORAGE_ADDRESS,
  },
};
use primitives::account::StoragePoints;
use std::sync::atomic::Ordering;
use xcfx_state::{
  State,
  account::{SponsorInfo, StoragePointsState},
  database::StorageOwner,
};

type RemoteValue = (Vec<u8>, Option<StateValue>);

fn account_value(account: Account) -> RemoteValue {
  let address = account.address();
  (
    StorageKey::new_account_key(&address.address)
      .with_space(address.space)
      .to_key_bytes(),
    Some(StateValue::Account(Arc::new(account))),
  )
}

fn storage_value(address: Address, key: &[u8], value: StateValue) -> RemoteValue {
  (
    StorageKey::new_storage_key(&address, key)
      .with_native_space()
      .to_key_bytes(),
    Some(value),
  )
}

fn builtin_account(address: Address) -> Account {
  let mut account = Account::new_empty(&address.with_native_space());
  account.nonce = U256::one();
  account
}

fn conversion_values(account: Account, ratio: U256) -> Vec<RemoteValue> {
  vec![
    account_value(account),
    account_value(builtin_account(SYSTEM_STORAGE_ADDRESS)),
    storage_value(
      SYSTEM_STORAGE_ADDRESS,
      &storage_point_prop(),
      StateValue::Storage(StorageEntry::new(ratio, None)),
    ),
  ]
}

fn inherited_contract() -> Account {
  let mut account = Account::new_empty(&Address::from([0x80; 20]).with_native_space());
  account.balance = 5.into();
  account.code_hash = keccak([0x00]);
  account.sponsor_info.sponsor_for_collateral = Address::from([0x10; 20]);
  account.sponsor_info.sponsor_balance_for_collateral = 10.into();
  account.sponsor_info.storage_points = StoragePointsState::Unknown;
  account
}

fn parent(remote: &MockFork) -> Arc<ForkStateVersion> {
  let mut globals = [U256::zero(); TOTAL_GLOBAL_PARAMS];
  globals[TotalIssued::ID] = U256::from(ONE_CFX_IN_DRIP) * 100;
  Arc::new(ForkStateVersion::new(remote.client.clone(), globals))
}

#[test]
fn lazy_initialization_preserves_local_amounts_checkpoints_and_versions() {
  for initialized in [false, true] {
    let account = inherited_contract();
    let address = *account.address();
    let sponsor = account.sponsor_info.sponsor_for_collateral;
    let remote = MockFork::new(
      conversion_values(account.clone(), ONE_CFX_IN_DRIP.into()),
      Some((address.address, Ok(initialized))),
    );
    let parent = parent(&remote);
    let sibling = ForkStateCandidate::new(Arc::clone(&parent));
    let mut candidate = ForkStateCandidate::new(Arc::clone(&parent));
    let mut state = State::borrow_database(&mut candidate).unwrap();
    state.add_balance(&address, &7.into()).unwrap();
    // One drip at R=D rounds to zero in both initialization branches.
    assert_eq!(
      state
        .set_sponsor_for_collateral(&address.address, &sponsor, &11.into(), true)
        .unwrap(),
      U256::zero()
    );
    let before = SponsorInfo {
      sponsor_balance_for_collateral: 11.into(),
      ..account.sponsor_info.clone()
    };
    assert_eq!(
      state.sponsor_info(&address.address).unwrap(),
      Some(before.clone())
    );
    assert_eq!(remote.initialization_reads.load(Ordering::SeqCst), 0);
    let saved = state.save();
    let issued = state.total_issued_tokens();
    let converted = U256::from(if initialized { 2 } else { 0 });
    let after = SponsorInfo {
      sponsor_balance_for_collateral: U256::from(15) - converted,
      storage_points: if initialized {
        StoragePointsState::Initialized(StoragePoints {
          unused: converted,
          used: U256::zero(),
        })
      } else {
        StoragePointsState::Uninitialized
      },
      ..before.clone()
    };

    state.checkpoint();
    assert_eq!(
      state
        .set_sponsor_for_collateral(&address.address, &sponsor, &15.into(), true)
        .unwrap(),
      converted
    );
    assert_eq!(
      state.sponsor_info(&address.address).unwrap(),
      Some(after.clone())
    );
    assert_eq!(state.total_issued_tokens(), issued - converted);
    state.revert_to_checkpoint();
    assert_eq!(
      state.sponsor_info(&address.address).unwrap(),
      Some(before.clone())
    );
    assert_eq!(state.total_issued_tokens(), issued);

    state
      .set_sponsor_for_collateral(&address.address, &sponsor, &15.into(), true)
      .unwrap();
    state.restore(saved);
    assert_eq!(state.balance(&address).unwrap(), 12.into());
    assert_eq!(state.sponsor_info(&address.address).unwrap(), Some(before));
    assert_eq!(state.total_issued_tokens(), issued);

    state
      .set_sponsor_for_collateral(&address.address, &sponsor, &15.into(), true)
      .unwrap();
    state.apply_changes_to_storage(None).unwrap();
    drop(state);
    let child = ForkStateCandidate::new(Arc::new(candidate.into_version()));
    let mut expected = account.clone();
    expected.balance = 12.into();
    expected.sponsor_info = after;
    assert_eq!(child.get_account(&address).unwrap(), Some(expected));
    assert_eq!(sibling.get_account(&address).unwrap().unwrap(), account);
    assert_eq!(
      ForkStateCandidate::new(parent)
        .get_account(&address)
        .unwrap()
        .unwrap(),
      account
    );
  }
}

#[test]
fn unrelated_writeback_preserves_unknown_and_recreation_establishes_its_own_flag() {
  let account = inherited_contract();
  let address = *account.address();
  let remote = MockFork::new(
    conversion_values(account.clone(), ONE_CFX_IN_DRIP.into()),
    None,
  );
  let original = parent(&remote);
  let mut candidate = ForkStateCandidate::new(Arc::clone(&original));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  state.add_balance(&address, &7.into()).unwrap();
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  let changed = Arc::new(candidate.into_version());
  let mut candidate = ForkStateCandidate::new(Arc::clone(&changed));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  assert_eq!(
    state
      .sponsor_info(&address.address)
      .unwrap()
      .unwrap()
      .storage_points,
    StoragePointsState::Unknown
  );
  assert_eq!(state.balance(&address).unwrap(), 12.into());
  state.remove_contract(&address).unwrap();
  state
    .new_contract_with_admin(
      &address,
      &Address::zero(),
      U256::zero(),
      Some(primitives::storage::STORAGE_LAYOUT_REGULAR_V0),
      true,
    )
    .unwrap();
  state
    .init_code(&address, vec![0x00], Address::zero(), H256::zero())
    .unwrap();
  assert_eq!(
    state
      .set_sponsor_for_collateral(
        &address.address,
        &Address::from([0x10; 20]),
        &4.into(),
        true
      )
      .unwrap(),
    2.into()
  );
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  assert_eq!(
    candidate
      .get_account(&address)
      .unwrap()
      .unwrap()
      .sponsor_info
      .storage_points,
    StoragePointsState::Initialized(StoragePoints {
      unused: 2.into(),
      used: U256::zero()
    })
  );
  assert_eq!(
    ForkStateCandidate::new(changed)
      .get_account(&address)
      .unwrap()
      .unwrap()
      .sponsor_info
      .storage_points,
    StoragePointsState::Unknown
  );
  assert_eq!(
    ForkStateCandidate::new(original)
      .get_account(&address)
      .unwrap()
      .unwrap(),
    account
  );
}

#[test]
fn ambiguous_empty_account_deletion_resolves_only_at_writeback() {
  for initialized in [
    Ok(false),
    Ok(true),
    Err(ForkReadError::Unavailable("injected deletion failure")),
  ] {
    let mut account = Account::new_empty(&Address::from([0x80; 20]).with_native_space());
    account.balance = U256::one();
    account.sponsor_info.storage_points = StoragePointsState::Unknown;
    let address = *account.address();
    let remote = MockFork::new(
      vec![account_value(account.clone())],
      Some((address.address, initialized.clone())),
    );
    let parent = parent(&remote);
    let mut candidate = ForkStateCandidate::new(Arc::clone(&parent));
    let mut state = State::borrow_database(&mut candidate).unwrap();
    state.sub_balance(&address, &U256::one()).unwrap();
    assert_eq!(remote.initialization_reads.load(Ordering::SeqCst), 0);
    let result = state.apply_changes_to_storage(None);
    assert_eq!(remote.initialization_reads.load(Ordering::SeqCst), 1);
    drop(state);
    match initialized {
      Ok(false) => {
        result.unwrap();
        assert!(candidate.get_account(&address).unwrap().is_none());
      }
      Ok(true) => {
        result.unwrap();
        let retained = candidate.get_account(&address).unwrap().unwrap();
        assert_eq!(retained.balance, U256::zero());
        assert_eq!(
          retained.sponsor_info.storage_points,
          StoragePointsState::Initialized(StoragePoints::default())
        );
      }
      Err(_) => assert!(result.is_err()),
    }
    assert_eq!(
      ForkStateCandidate::new(parent)
        .get_account(&address)
        .unwrap()
        .unwrap(),
      account
    );
  }
}

#[test]
fn collateral_settlement_uses_the_inherited_flag_and_current_balance() {
  let chain = ChainSpec::new(ChainIds::default());
  let spec = chain.machine().spec(1, 1);
  let unit = U256::from(ONE_CFX_IN_DRIP);
  let cost = unit / 16;
  for initialized in [false, true] {
    let mut account = inherited_contract();
    account.sponsor_info.sponsor_balance_for_collateral = unit;
    let address = *account.address();
    let remote = MockFork::new(
      conversion_values(account, unit),
      Some((address.address, Ok(initialized))),
    );
    let mut candidate = ForkStateCandidate::new(parent(&remote));
    let mut state = State::borrow_database(&mut candidate).unwrap();
    let mut substate = Substate::new();
    state
      .set_storage(
        &address,
        vec![0; 32],
        U256::one(),
        address.address,
        &mut substate,
      )
      .unwrap();
    state
      .settle_collateral_for_all(&substate, &mut (), &spec, false)
      .unwrap()
      .unwrap();
    let converted = if initialized { U256::zero() } else { unit / 2 };
    assert_eq!(
      state
        .sponsor_balance_for_collateral(&address.address)
        .unwrap(),
      unit - converted - cost
    );
    assert_eq!(
      state
        .available_storage_points_for_collateral(&address.address)
        .unwrap(),
      converted
    );
    assert_eq!(
      state
        .token_collateral_for_storage(&address.address)
        .unwrap(),
      cost
    );
    assert_eq!(remote.initialization_reads.load(Ordering::SeqCst), 1);
  }

  let mut account = inherited_contract();
  account.sponsor_info.sponsor_balance_for_collateral = U256::one();
  let address = *account.address();
  let remote = MockFork::new(conversion_values(account, unit), None);
  let mut candidate = ForkStateCandidate::new(parent(&remote));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let mut substate = Substate::new();
  state
    .set_storage(
      &address,
      vec![0; 32],
      U256::one(),
      address.address,
      &mut substate,
    )
    .unwrap();
  state
    .settle_collateral_for_all(&substate, &mut (), &spec, true)
    .unwrap()
    .unwrap();
  assert_eq!(
    state
      .sponsor_info(&address.address)
      .unwrap()
      .unwrap()
      .storage_points,
    StoragePointsState::Initialized(StoragePoints::default())
  );
  assert_eq!(
    state
      .sponsor_balance_for_collateral(&address.address)
      .unwrap(),
    U256::one()
  );
}

fn whitelist_key(contract: Address, user: Address) -> Vec<u8> {
  [contract.as_bytes(), user.as_bytes()].concat()
}

#[test]
fn local_whitelist_ownership_and_eligibility_survive_version_restore() {
  let chain = ChainSpec::new(ChainIds::default());
  let spec = chain.machine().spec(1, 1);
  let contract = Address::from([0x80; 20]);
  let user = Address::from([0x12; 20]);
  let payer = Address::from([0x10; 20]);
  let other = Address::from([0x11; 20]);
  let builtin = SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS;
  let all_key = whitelist_key(contract, Address::zero());
  let personal_key = whitelist_key(contract, user);
  let unit = U256::from(ONE_CFX_IN_DRIP);
  let cost = unit / 16;
  let mut values = vec![
    account_value(builtin_account(builtin)),
    storage_value(
      builtin,
      &all_key,
      StateValue::Storage(StorageEntry::with_unknown_owner(U256::one())),
    ),
    storage_value(builtin, &personal_key, StateValue::HiddenWhitelistEntry),
  ];
  for address in [payer, other] {
    let mut account = Account::new_empty(&address.with_native_space());
    account.balance = unit;
    values.push(account_value(account));
  }
  let remote = MockFork::new(values, None);
  let parent = parent(&remote);
  let mut candidate = ForkStateCandidate::new(Arc::clone(&parent));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let balances = |state: &State<'_>| {
    (
      state.balance(&payer.with_native_space()).unwrap(),
      state.balance(&other.with_native_space()).unwrap(),
    )
  };
  let entry = |state: &State<'_>| {
    state
      .storage_entry_at(&builtin.with_native_space(), &personal_key)
      .unwrap()
  };

  assert!(state.check_contract_whitelist(&contract, &user).unwrap());
  assert_eq!(
    remote.state_reads(StorageKey::new_storage_key(&builtin, &personal_key).with_native_space()),
    0
  );
  state.checkpoint();
  let mut substate = Substate::new();
  state
    .remove_from_contract_whitelist(contract, payer, Address::zero(), &mut substate)
    .unwrap();
  assert!(!state.check_contract_whitelist(&contract, &user).unwrap());
  state.revert_to_checkpoint();
  assert!(state.check_contract_whitelist(&contract, &user).unwrap());

  let mut substate = Substate::new();
  state
    .remove_from_contract_whitelist(contract, payer, Address::zero(), &mut substate)
    .unwrap();
  state
    .remove_from_contract_whitelist(contract, payer, user, &mut substate)
    .unwrap();
  state
    .add_to_contract_whitelist(contract, payer, user, &mut substate)
    .unwrap();
  state
    .settle_collateral_for_all(&substate, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  let added = Arc::new(candidate.into_version());

  let mut candidate = ForkStateCandidate::new(Arc::clone(&added));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let saved = state.save();
  let before = (
    (unit - cost, unit),
    StorageEntry::new(U256::one(), Some(payer)),
  );
  assert_eq!((balances(&state), entry(&state)), before);

  // SponsorControl resets a known owner even when the stored value stays one.
  let mut substate = Substate::new();
  state
    .add_to_contract_whitelist(contract, other, user, &mut substate)
    .unwrap();
  state
    .settle_collateral_for_all(&substate, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  assert_eq!(
    (balances(&state), entry(&state)),
    (
      (unit, unit - cost),
      StorageEntry::new(U256::one(), Some(other))
    )
  );
  state.restore(saved);
  assert_eq!((balances(&state), entry(&state)), before);

  let mut substate = Substate::new();
  state
    .remove_from_contract_whitelist(contract, other, user, &mut substate)
    .unwrap();
  state
    .settle_collateral_for_all(&substate, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  assert!(!state.check_contract_whitelist(&contract, &user).unwrap());
  assert_eq!(balances(&state), (unit, unit));
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  let removed = Arc::new(candidate.into_version());

  for (version, eligible, balance) in [
    (parent, true, unit),
    (added, true, unit - cost),
    (removed, false, unit),
  ] {
    let mut candidate = ForkStateCandidate::new(version);
    let state = State::borrow_database(&mut candidate).unwrap();
    assert_eq!(
      state.check_contract_whitelist(&contract, &user).unwrap(),
      eligible
    );
    assert_eq!(balances(&state), (balance, unit));
  }
}

#[test]
fn historical_slot_lifecycle_keeps_collateral_attribution() {
  let chain = ChainSpec::new(ChainIds::default());
  let spec = chain.machine().spec(1, 1);
  let contract = inherited_contract();
  let address = *contract.address();
  let key = vec![0; 32];
  let payer = Address::from([0x12; 20]);
  let funds = U256::from(ONE_CFX_IN_DRIP);
  let cost = funds / 16;
  let mut account = Account::new_empty(&payer.with_native_space());
  account.balance = funds;
  let remote = MockFork::new(
    vec![
      account_value(contract),
      account_value(account),
      storage_value(
        address.address,
        &key,
        StateValue::Storage(StorageEntry::with_unknown_owner(7.into())),
      ),
    ],
    None,
  );
  let mut globals = [U256::zero(); TOTAL_GLOBAL_PARAMS];
  globals[TotalIssued::ID] = funds * 100;
  globals[TotalStorage::ID] = cost;
  let original = Arc::new(ForkStateVersion::new(remote.client.clone(), globals));
  let mut candidate = ForkStateCandidate::new(original);
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let mut transaction = Substate::new();

  // Clearing and restoring within one transaction keeps the historical payment.
  state
    .set_storage(&address, key.clone(), U256::zero(), payer, &mut transaction)
    .unwrap();
  state
    .set_storage(&address, key.clone(), 7.into(), payer, &mut transaction)
    .unwrap();
  assert_eq!(
    state.storage_entry_at(&address, &key).unwrap().owner(),
    StorageOwner::Unknown
  );
  assert_eq!(transaction.get_collateral_change(&payer), (0, 0));
  state
    .set_storage(&address, key.clone(), U256::zero(), payer, &mut transaction)
    .unwrap();
  state
    .settle_collateral_for_all(&transaction, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  assert_eq!(state.total_storage_tokens(), cost);
  state.apply_changes_to_storage(None).unwrap();
  drop(state);

  // Reopen after deletion: restoring even the same base value is a local addition.
  let mut candidate = ForkStateCandidate::new(Arc::new(candidate.into_version()));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let mut transaction = Substate::new();
  state
    .set_storage(&address, key.clone(), 7.into(), payer, &mut transaction)
    .unwrap();
  state
    .settle_collateral_for_all(&transaction, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  assert_eq!(
    state.balance(&payer.with_native_space()).unwrap(),
    funds - cost
  );
  assert_eq!(state.token_collateral_for_storage(&payer).unwrap(), cost);
  assert_eq!(state.total_storage_tokens(), cost * 2);
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  let reopened = ForkStateCandidate::new(Arc::new(candidate.into_version()));
  assert_eq!(
    reopened.get_storage_entry(&address, &key).unwrap(),
    Some(StorageEntry::new(7.into(), Some(payer)))
  );
}

fn create_contract_with_storage(
  state: &mut State<'_>,
  contract: &cfx_types::AddressWithSpace,
  payer: Address,
  spec: &cfx_vm_types::Spec,
) {
  use cfx_parameters::staking::code_collateral_units;
  use primitives::storage::STORAGE_LAYOUT_REGULAR_V0;

  state
    .new_contract_with_admin(
      contract,
      &Address::zero(),
      U256::zero(),
      Some(STORAGE_LAYOUT_REGULAR_V0),
      true,
    )
    .unwrap();
  let mut occupied = Substate::new();
  // The VM reports code occupancy separately from State::init_code.
  occupied.record_storage_occupy(&payer, code_collateral_units(1));
  state
    .init_code(contract, vec![0x00], payer, H256::zero())
    .unwrap();
  state
    .set_storage(contract, vec![0; 32], 7.into(), payer, &mut occupied)
    .unwrap();
  state
    .settle_collateral_for_all(&occupied, &mut (), spec, false)
    .unwrap()
    .unwrap();
}

#[test]
fn new_contract_lifecycle_releases_local_records() {
  let chain = ChainSpec::new(ChainIds::default());
  let spec = chain.machine().spec(1, 1);
  let payer = Address::from([0x12; 20]);
  let contract = Address::from([0x81; 20]).with_native_space();
  let funds = U256::from(ONE_CFX_IN_DRIP);
  let mut account = Account::new_empty(&payer.with_native_space());
  account.balance = funds;
  let remote = MockFork::new(vec![account_value(account)], None);
  let mut candidate = ForkStateCandidate::new(parent(&remote));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  let code_hash = keccak([0x00]);
  create_contract_with_storage(&mut state, &contract, payer, &spec);
  assert_eq!(
    state.token_collateral_for_storage(&payer).unwrap(),
    funds / 2 + funds / 16
  );

  // Follow the executor's release, settlement, removal and writeback order.
  let mut released = Substate::new();
  state
    .record_contract_collateral_release(&contract, &mut released, spec.cip131)
    .unwrap();
  state
    .settle_collateral_for_all(&released, &mut (), &spec, false)
    .unwrap()
    .unwrap();
  state.remove_contract(&contract).unwrap();
  assert_eq!(state.balance(&payer.with_native_space()).unwrap(), funds);
  assert_eq!(
    state.token_collateral_for_storage(&payer).unwrap(),
    U256::zero()
  );
  assert_eq!(state.total_storage_tokens(), U256::zero());
  state.apply_changes_to_storage(None).unwrap();
  drop(state);
  let reopened = ForkStateCandidate::new(Arc::new(candidate.into_version()));
  assert!(reopened.get_account(&contract).unwrap().is_none());
  assert!(reopened.get_code(&contract, &code_hash).unwrap().is_none());
  assert!(
    reopened
      .get_storage_entry(&contract, &[0; 32])
      .unwrap()
      .is_none()
  );
}

#[test]
fn committed_new_contract_keeps_unwritten_storage_local() {
  let chain = ChainSpec::new(ChainIds::default());
  let spec = chain.machine().spec(1, 1);
  let payer = Address::from([0x12; 20]);
  let contract = Address::from([0x81; 20]).with_native_space();
  let written_key = [0; 32];
  let unwritten_key = [1; 32];
  let mut payer_account = Account::new_empty(&payer.with_native_space());
  payer_account.balance = U256::from(ONE_CFX_IN_DRIP);
  let inherited = inherited_contract();
  let inherited_address = *inherited.address();
  let remote = MockFork::new(
    vec![
      account_value(payer_account),
      account_value(inherited),
      storage_value(
        inherited_address.address,
        &unwritten_key,
        StateValue::Storage(StorageEntry::with_unknown_owner(9.into())),
      ),
    ],
    None,
  );
  let original = parent(&remote);
  let mut candidate = ForkStateCandidate::new(Arc::clone(&original));
  let mut state = State::borrow_database(&mut candidate).unwrap();
  create_contract_with_storage(&mut state, &contract, payer, &spec);
  // An updated historical account must continue to inherit its remote storage.
  state.set_nonce(&inherited_address, &U256::one()).unwrap();
  state.apply_changes_to_storage(None).unwrap();
  drop(state);

  let version = Arc::new(candidate.into_version());
  let mut reopened = ForkStateCandidate::new(Arc::clone(&version));
  let state = State::borrow_database(&mut reopened).unwrap();
  assert_eq!(state.storage_at(&contract, &written_key).unwrap(), 7.into());
  assert_eq!(
    state.storage_at(&contract, &unwritten_key).unwrap(),
    U256::zero()
  );
  assert_eq!(state.code(&contract).unwrap().unwrap().as_slice(), &[0x00]);
  assert_eq!(
    remote.state_reads(
      StorageKey::new_storage_key(&contract.address, &unwritten_key).with_native_space()
    ),
    0
  );
  assert_eq!(
    version
      .native_storage(contract.address, H256::from(unwritten_key))
      .unwrap(),
    None
  );
  assert_eq!(
    state
      .storage_at(&inherited_address, &unwritten_key)
      .unwrap(),
    9.into()
  );
  assert_eq!(
    remote.state_reads(
      StorageKey::new_storage_key(&inherited_address.address, &unwritten_key).with_native_space()
    ),
    1
  );
  assert!(
    ForkStateCandidate::new(original)
      .get_account(&contract)
      .unwrap()
      .is_none()
  );
}
