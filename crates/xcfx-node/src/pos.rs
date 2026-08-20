//! Applies one deterministic Conflux PoS view transition.
use cfx_types::{Address, H256};
use diem_crypto::{HashValue, traits::VRFProof};
use diem_types::{
  account_address::AccountAddress,
  block_info::{
    GENESIS_EPOCH, GENESIS_ROUND, GENESIS_TIMESTAMP_USECS, GENESIS_VERSION, PivotBlockDecision,
  },
  committed_block::CommittedBlock,
  contract_event::ContractEvent,
  epoch_state::EpochState,
  term_state::{ElectionEvent, NodeID, PosState, pos_state_config::PosStateConfig},
  transaction::{
    ElectionPayload, RawTransaction, RegisterPayload, RetirePayload, TransactionPayload,
    UpdateVotingPowerPayload,
  },
};
use pow_types::StakingEvent;
use primitives::{pos::PosBlockId, transaction::native_transaction::NativeTransaction};

pub(crate) const GENESIS_POS_REFERENCE: PosBlockId = H256([0; 32]);

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PosEnvInput {
  pub(crate) pos_view: u64,
  pub(crate) finalized_epoch: u64,
}

pub(crate) struct PosTransitionInput<'a> {
  pub(crate) elections: &'a [ElectionPayload],
  pub(crate) pivot: Option<PivotAdvanceInput<'a>>,
}
pub(crate) struct PivotAdvanceInput<'a> {
  pub(crate) decision: &'a PivotBlockDecision,
  pub(crate) staking_events: &'a [StakingEvent],
}

pub(crate) struct PosTransitionOutcome {
  pub(crate) state: PosState,
  pub(crate) unlock_events: Vec<ContractEvent>,
  pub(crate) next_epoch_state: Option<EpochState>,
}

pub(crate) struct CommittedPosState {
  metadata: CommittedBlock,
  state: PosState,
  next_epoch_state: Option<EpochState>,
}

impl CommittedPosState {
  pub(crate) fn reference(&self) -> PosBlockId {
    H256::from(self.metadata.hash.as_ref())
  }

  pub(crate) fn env_input(&self, reference: &PosBlockId) -> Option<PosEnvInput> {
    if *reference != self.reference() {
      return None;
    }

    Some(PosEnvInput {
      pos_view: self.metadata.view,
      finalized_epoch: self.metadata.pivot_decision.height,
    })
  }
}

/// Converts a validated Local Genesis view-0 state into the committed view-1 record.
pub(crate) fn bootstrap_genesis_pos_state(
  definition: &GenesisPosDefinition,
  config: &PosStateConfig,
  genesis_pivot: PivotBlockDecision,
) -> CommittedPosState {
  let initial_state = definition.initial_pos_state(config, genesis_pivot);

  assert_eq!(initial_state.current_view(), 0);
  assert_eq!(initial_state.pivot_decision().height, 0);

  let outcome = advance_pos_state(
    &initial_state,
    config,
    PosTransitionInput {
      elections: &[],
      pivot: None,
    },
  );

  assert!(outcome.unlock_events.is_empty());

  let next_epoch_state = outcome
    .next_epoch_state
    .expect("Genesis PoS bootstrap must create epoch 1");

  let state = outcome.state;
  let metadata = CommittedBlock {
    hash: HashValue::new(GENESIS_POS_REFERENCE.0),
    miner: None,
    parent_hash: HashValue::zero(),
    epoch: GENESIS_EPOCH,
    round: GENESIS_ROUND,
    pivot_decision: state.pivot_decision().clone(),
    version: GENESIS_VERSION,
    timestamp: GENESIS_TIMESTAMP_USECS,
    view: state.current_view(),
    is_skipped: false,
  };

  CommittedPosState {
    metadata,
    state,
    next_epoch_state: Some(next_epoch_state),
  }
}

pub(crate) fn advance_pos_state(
  parent: &PosState,
  config: &PosStateConfig,
  input: PosTransitionInput<'_>,
) -> PosTransitionOutcome {
  let mut unlock_events = parent.get_unlock_events();
  unlock_events.sort_by(|left, right| left.event_data().cmp(right.event_data()));

  let election_events = input
    .elections
    .iter()
    .enumerate()
    .map(|(index, payload)| {
      parent
        .validate_election_with_config(config, payload)
        .unwrap_or_else(|error| {
          panic!(
            "PoS transition invariant violated while validating election \
               {index}: {error:#}"
          )
        });

      let vrf_output = payload.vrf_proof.to_hash().unwrap_or_else(|error| {
        panic!(
          "PoS transition invariant violated while materializing election \
             {index}: {error:#}"
        )
      });

      ElectionEvent::new(
        payload.public_key.clone(),
        payload.vrf_public_key.clone(),
        vrf_output,
        payload.target_term,
      )
    })
    .collect::<Vec<_>>();

  let pivot = input.pivot.map(|pivot| {
    assert!(
      pivot.decision.height > parent.pivot_decision().height,
      "PoS transition invariant violated: pivot height {} must be greater \
         than parent pivot height {}",
      pivot.decision.height,
      parent.pivot_decision().height,
    );

    let actions = pivot
      .staking_events
      .iter()
      .enumerate()
      .map(|(index, event)| StakingAction::from_event(event, index))
      .collect::<Vec<_>>();

    (pivot.decision.clone(), actions)
  });

  let mut state = parent.clone();

  for (index, event) in election_events.iter().enumerate() {
    state
      .new_node_elected_with_config(config, event)
      .unwrap_or_else(|error| {
        panic!(
          "PoS transition invariant violated while applying election \
             {index}: {error:#}"
        )
      });
  }

  if let Some((decision, actions)) = pivot {
    for (index, action) in actions.into_iter().enumerate() {
      action.apply(&mut state, config, index);
    }

    state.set_pivot_decision(decision);
  }

  let next_epoch_state = state.next_view_with_config(config).unwrap_or_else(|error| {
    panic!(
      "PoS transition invariant violated while advancing parent view {}: \
         {error:#}",
      parent.current_view(),
    )
  });

  PosTransitionOutcome {
    state,
    unlock_events,
    next_epoch_state,
  }
}

enum StakingAction {
  Register(RegisterPayload),
  UpdateVotingPower(UpdateVotingPowerPayload),
  Retire(RetirePayload),
}

impl StakingAction {
  fn from_event(event: &StakingEvent, event_index: usize) -> Self {
    // The converter only copies sender into the discarded transaction shell.
    let sender = AccountAddress::new([0; AccountAddress::LENGTH]);
    let payload = RawTransaction::from_staking_event(event, sender)
      .unwrap_or_else(|error| {
        panic!(
          "PoS transition invariant violated while converting staking event \
             {event_index}: {error:#}"
        )
      })
      .into_payload();

    match payload {
      TransactionPayload::Register(payload) => Self::Register(payload),
      TransactionPayload::UpdateVotingPower(payload) => Self::UpdateVotingPower(payload),
      TransactionPayload::Retire(payload) => Self::Retire(payload),
      _ => unreachable!("from_staking_event returned a non-staking transaction payload"),
    }
  }

  fn apply(self, state: &mut PosState, config: &PosStateConfig, event_index: usize) {
    let (action, result) = match self {
      Self::Register(payload) => (
        "register",
        state.register_node(NodeID::new(payload.public_key, payload.vrf_public_key)),
      ),
      Self::UpdateVotingPower(payload) => (
        "update voting power",
        state.update_voting_power_with_config(config, &payload.node_address, payload.voting_power),
      ),
      Self::Retire(payload) => (
        "retire",
        state.retire_node_with_config(config, &payload.node_id, payload.votes),
      ),
    };

    result.unwrap_or_else(|error| {
      panic!(
        "PoS transition invariant violated while applying staking event \
           {event_index} ({action}): {error:#}"
      )
    });
  }
}

pub(crate) struct GenesisPosDefinition {
  pub(crate) initial_seed: H256,
  pub(crate) initial_nodes: Vec<GenesisPosNode>,
  pub(crate) initial_committee: Vec<(AccountAddress, u64)>,
}

pub(crate) struct GenesisPosNode {
  pub(crate) execution_address: Address,
  pub(crate) node_id: NodeID,
  pub(crate) voting_power: u64,
  pub(crate) register_transaction: NativeTransaction,
}

impl GenesisPosDefinition {
  pub(crate) fn initial_pos_state(
    &self,
    config: &PosStateConfig,
    genesis_pivot: PivotBlockDecision,
  ) -> PosState {
    let initial_nodes = self
      .initial_nodes
      .iter()
      .map(|node| (node.node_id.clone(), node.voting_power))
      .collect();

    PosState::new_with_config(
      config,
      self.initial_seed.as_bytes().to_vec(),
      initial_nodes,
      self.initial_committee.clone(),
      genesis_pivot,
    )
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use cfx_types::H256;
  use diem_crypto::{
    PrivateKey, Uniform, ValidCryptoMaterial, bls::BLSPrivateKey, ec_vrf::EcVrfPrivateKey,
  };
  use rand::{SeedableRng, rngs::StdRng};

  struct TestNode {
    node_id: NodeID,
    bls_public_key: Vec<u8>,
    vrf_public_key: Vec<u8>,
  }

  fn pivot_decision(height: u64, hash_byte: u8) -> PivotBlockDecision {
    PivotBlockDecision {
      height,
      block_hash: H256([hash_byte; 32]),
    }
  }

  fn empty_state(config: &PosStateConfig) -> PosState {
    PosState::new_with_config(
      config,
      b"test-pos-seed".to_vec(),
      Vec::new(),
      Vec::new(),
      pivot_decision(0, 0),
    )
  }

  fn test_node(seed: u64) -> TestNode {
    let mut rng = StdRng::seed_from_u64(seed);
    let bls_public_key = BLSPrivateKey::generate(&mut rng).public_key();
    let vrf_public_key = EcVrfPrivateKey::generate(&mut rng).public_key();
    let node_id = NodeID::new(bls_public_key.clone(), vrf_public_key.clone());

    TestNode {
      node_id,
      bls_public_key: bls_public_key.to_bytes(),
      vrf_public_key: vrf_public_key.to_bytes(),
    }
  }

  #[test]
  fn applies_staking_events_in_execution_order_with_pivot() {
    let config = PosStateConfig::default();
    let parent = empty_state(&config);

    let node = test_node(7);
    let address = H256::from_slice(node.node_id.addr.as_ref());

    let staking_events = [
      StakingEvent::Register(address, node.bls_public_key, node.vrf_public_key),
      StakingEvent::IncreaseStake(address, 7),
    ];
    let decision = pivot_decision(1, 1);

    let outcome = advance_pos_state(
      &parent,
      &config,
      PosTransitionInput {
        elections: &[],
        pivot: Some(PivotAdvanceInput {
          decision: &decision,
          staking_events: &staking_events,
        }),
      },
    );

    assert_eq!(outcome.state.pivot_decision(), &decision);
    assert_eq!(outcome.state.current_view(), 1);

    let node_data = outcome
      .state
      .account_node_data(node.node_id.addr)
      .expect("register event must create the node");
    assert_eq!(node_data.lock_status().available_votes(), 7);
  }

  #[test]
  fn bootstraps_genesis_as_a_committed_pos_version() {
    let config = PosStateConfig::default();
    let genesis_pivot = pivot_decision(0, 9);
    let node = test_node(11);
    let node_address = node.node_id.addr.clone();
    let voting_power = 7;

    let definition = GenesisPosDefinition {
      initial_seed: H256([9; 32]),
      initial_nodes: vec![GenesisPosNode {
        execution_address: Address::zero(),
        node_id: node.node_id.clone(),
        voting_power,
        register_transaction: NativeTransaction::default(),
      }],
      initial_committee: vec![(node_address.clone(), voting_power)],
    };

    let committed = bootstrap_genesis_pos_state(&definition, &config, genesis_pivot.clone());

    let node_data = committed
      .state
      .account_node_data(node_address)
      .expect("Genesis node must exist in committed PoS state");

    assert_eq!(
      (
        committed.reference(),
        committed.metadata.view,
        committed.state.current_view(),
        node_data.lock_status().available_votes(),
      ),
      (H256::zero(), 1, 1, voting_power),
    );
    assert_eq!(committed.metadata.pivot_decision, genesis_pivot);
    assert_eq!(committed.state.pivot_decision(), &genesis_pivot);
    assert_eq!(
      committed.next_epoch_state.as_ref().map(|state| state.epoch),
      Some(1),
    );
  }
}
