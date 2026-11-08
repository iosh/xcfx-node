use std::{
  collections::HashMap,
  num::NonZeroU64,
  sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
  },
  task::{Context, Poll},
  thread::JoinHandle,
};

use alloy_json_rpc::RequestPacket;
use alloy_rpc_client::RpcClient;
use alloy_transport::mock::{Asserter, MockTransport};
use cfx_addr::Network;
use cfx_parameters::internal_contract_addresses::SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS;
use cfx_rpc_cfx_types::{Block, BlockTransactions, Bytes, RpcAddress};
use cfx_types::{Address, H256, U256};
use diem_types::block_info::PivotBlockDecision;
use hex_literal::hex;
use primitives::{SkipInputCheck, StorageKey, StorageKeyWithSpace};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tower_service::Service;
use xcfx_state::database::StorageEntry;

use crate::{chain_spec::ChainIds, rpc_client::ConfluxRpcClient};

use super::{
  ForkBase, ForkClient, ForkReadError, NetworkIdentity, StateValue,
  client::ReadRequest,
  rpc::{ForkRpc, StateKey},
};

pub(super) fn base() -> ForkBase {
  ForkBase {
    latest_confirmed: 7,
    latest_finalized: 7,
    latest_checkpoint: 0,
    network: NetworkIdentity {
      chain_ids: ChainIds::default(),
      genesis_hash: H256::zero(),
    },
    era_epoch_count: NonZeroU64::new(100).unwrap(),
    epoch_height: 7,
    pivot_hash: H256::from_low_u64_be(0xabc),
    pivot_block_number: 12,
    timestamp: 100,
    author: Address::zero(),
    header_gas_limit: 30_000_000.into(),
    base_price: None,
    pos_reference: Default::default(),
    pos_view: 0,
    pivot_decision: PivotBlockDecision {
      height: 0,
      block_hash: H256::zero(),
    },
  }
}

pub(super) fn pivot(base: &ForkBase) -> Block {
  Block {
    hash: base.pivot_hash,
    parent_hash: H256::zero(),
    height: base.epoch_height.into(),
    miner: RpcAddress::try_from_h160(
      Address::zero(),
      Network::Id(base.network.chain_ids.network_id),
    )
    .unwrap(),
    deferred_state_root: H256::zero(),
    deferred_receipts_root: H256::zero(),
    deferred_logs_bloom_hash: H256::zero(),
    blame: 0.into(),
    transactions_root: H256::zero(),
    epoch_number: Some(base.epoch_height.into()),
    block_number: Some(base.pivot_block_number.into()),
    gas_limit: base.header_gas_limit,
    gas_used: Some(U256::zero()),
    base_fee_per_gas: None,
    timestamp: base.timestamp.into(),
    difficulty: U256::one(),
    pow_quality: None,
    referee_hashes: vec![],
    adaptive: false,
    nonce: U256::zero(),
    transactions: BlockTransactions::Hashes(vec![]),
    size: None,
    custom: vec![],
    pos_reference: Some(base.pos_reference),
  }
}

pub(super) fn rpc(asserter: &Asserter) -> ForkRpc {
  ForkRpc {
    base: Arc::new(base()),
    rpc: ConfluxRpcClient::new(
      RpcClient::mocked(asserter.clone()),
      RpcClient::mocked(Asserter::new()),
    ),
  }
}

#[derive(Clone)]
struct RecordingTransport {
  inner: MockTransport,
  requests: Arc<Mutex<Vec<Value>>>,
}

impl Service<RequestPacket> for RecordingTransport {
  type Response = <MockTransport as Service<RequestPacket>>::Response;
  type Error = <MockTransport as Service<RequestPacket>>::Error;
  type Future = <MockTransport as Service<RequestPacket>>::Future;

  fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
    self.inner.poll_ready(context)
  }

  fn call(&mut self, request: RequestPacket) -> Self::Future {
    self
      .requests
      .lock()
      .unwrap()
      .push(serde_json::to_value(&request).unwrap());
    self.inner.call(request)
  }
}

fn bool_result(value: u8) -> Bytes {
  let mut word = vec![0; 32];
  word[31] = value;
  word.into()
}

#[tokio::test]
async fn all_address_sponsorship_does_not_invent_personal_entries() {
  let responses = Asserter::new();
  let rpc = rpc(&responses);
  let header = pivot(&rpc.base);
  let contract = Address::from([0x80; 20]);
  responses.push_success(&Some(header.clone()));
  responses.push_success(&bool_result(1));
  responses.push_success(&Some(header));
  let personal = rpc
    .state_value(StateKey::SponsorWhitelist {
      contract,
      user: Address::from([0x10; 20]),
    })
    .await
    .unwrap();
  assert!(matches!(personal, Some(StateValue::HiddenWhitelistEntry)));
  assert!(responses.read_q().is_empty());
}

#[tokio::test]
async fn personal_whitelist_is_exact_when_the_base_global_entry_is_off() {
  for present in [false, true] {
    let responses = Asserter::new();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
      inner: MockTransport::new(responses.clone()),
      requests: Arc::clone(&requests),
    };
    let rpc = ForkRpc {
      base: Arc::new(base()),
      rpc: ConfluxRpcClient::new(
        RpcClient::new(transport, true),
        RpcClient::mocked(Asserter::new()),
      ),
    };
    let header = pivot(&rpc.base);
    responses.push_success(&Some(header.clone()));
    responses.push_success(&bool_result(0));
    responses.push_success(&bool_result(u8::from(present)));
    responses.push_success(&Some(header));
    let value = rpc
      .state_value(StateKey::SponsorWhitelist {
        contract: Address::from([0x80; 20]),
        user: Address::from([0x10; 20]),
      })
      .await
      .unwrap();
    assert_eq!(
      value.map(|value| value.storage()),
      present.then(|| StorageEntry::with_unknown_owner(U256::one()))
    );
    assert!(responses.read_q().is_empty());

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for request in [requests.first().unwrap(), requests.last().unwrap()] {
      assert_eq!(request["method"], "cfx_getBlockByEpochNumber");
      assert_eq!(request["params"], json!(["0x7", false]));
    }
    let builtin = RpcAddress::try_from_h160(
      SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS,
      Network::Id(rpc.base.network.chain_ids.network_id),
    )
    .unwrap();
    for (request, signature, arguments) in [
      (
        &requests[1],
        "isAllWhitelisted(address)",
        hex!("0000000000000000000000008080808080808080808080808080808080808080").as_slice(),
      ),
      (
        &requests[2],
        "isWhitelisted(address,address)",
        hex!(
          "0000000000000000000000008080808080808080808080808080808080808080
         0000000000000000000000001010101010101010101010101010101010101010"
        )
        .as_slice(),
      ),
    ] {
      assert_eq!(request["method"], "cfx_call");
      assert_eq!(
        request["params"][0]["to"],
        serde_json::to_value(&builtin).unwrap()
      );
      assert_eq!(
        request["params"][1],
        json!({"blockHash": rpc.base.pivot_hash, "requirePivot": true})
      );
      let data: Bytes = serde_json::from_value(request["params"][0]["data"].clone()).unwrap();
      let data = data.into_vec();
      assert_eq!(&data[..4], &keccak_hash::keccak(signature).as_bytes()[..4]);
      assert_eq!(&data[4..], arguments);
    }
  }
}

#[tokio::test]
async fn whitelist_rpc_and_abi_errors_are_not_absence() {
  let responses = Asserter::new();
  let rpc = rpc(&responses);
  let key = StateKey::SponsorWhitelist {
    contract: Address::from([0x80; 20]),
    user: Address::zero(),
  };
  responses.push_success(&Some(pivot(&rpc.base)));
  responses.push_failure_msg("unavailable state");
  assert!(matches!(
    rpc.state_value(key).await,
    Err(ForkReadError::Rpc(_))
  ));

  responses.push_success(&Some(pivot(&rpc.base)));
  responses.push_success(&bool_result(2));
  assert!(matches!(
    rpc.state_value(key).await,
    Err(ForkReadError::Inconsistent(_))
  ));
}

#[test]
fn whitelist_keys_are_recognized_only_in_the_native_builtin() {
  use cfx_types::Space;
  let builtin = SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS;
  let contract = Address::from([0x80; 20]);
  let user = Address::from([0x10; 20]);
  let key = [contract.as_bytes(), user.as_bytes()].concat();
  assert_eq!(
    StateKey::try_from(StorageKey::new_storage_key(&builtin, &key).with_native_space()).unwrap(),
    StateKey::SponsorWhitelist { contract, user }
  );
  assert!(matches!(
    StateKey::try_from(StorageKey::new_storage_key(&builtin, &key).with_space(Space::Ethereum)),
    Err(ForkReadError::Unsupported(_))
  ));
  assert!(matches!(
    StateKey::try_from(StorageKey::new_storage_key(&contract, &key).with_native_space()),
    Err(ForkReadError::Unsupported(_))
  ));
  assert!(matches!(
    StateKey::try_from(StorageKey::new_storage_key(&builtin, &[0; 32]).with_native_space()),
    Ok(StateKey::Storage { .. })
  ));
}

#[tokio::test]
async fn a_pivot_change_discards_the_whitelist_result() {
  let responses = Asserter::new();
  let rpc = rpc(&responses);
  let mut header = pivot(&rpc.base);
  responses.push_success(&Some(header.clone()));
  responses.push_success(&bool_result(1));
  header.hash = H256::zero();
  responses.push_success(&Some(header));
  let result = rpc
    .state_value(StateKey::SponsorWhitelist {
      contract: Address::from([0x80; 20]),
      user: Address::zero(),
    })
    .await;
  assert!(matches!(result, Err(ForkReadError::Inconsistent(_))));
}

/// A fixed remote dataset for state/version tests; it performs no HTTP requests.
pub(crate) struct MockFork {
  pub(crate) client: ForkClient,
  pub(crate) initialization_reads: Arc<AtomicUsize>,
  reads: Arc<Mutex<HashMap<StateKey, usize>>>,
  stop: watch::Sender<bool>,
  task: Option<JoinHandle<()>>,
}

impl MockFork {
  pub(crate) fn new(
    values: Vec<(Vec<u8>, Option<StateValue>)>,
    initialization: Option<(Address, Result<bool, ForkReadError>)>,
  ) -> Self {
    let values = values
      .into_iter()
      .map(|(key, value)| {
        (
          StateKey::try_from(StorageKeyWithSpace::from_key_bytes::<SkipInputCheck>(&key)).unwrap(),
          value,
        )
      })
      .collect::<HashMap<_, _>>();
    let (requests, mut receiver) = mpsc::channel(64);
    let (stop, mut stopping) = watch::channel(false);
    let client = ForkClient::new(Arc::new(base()), requests, stopping.clone());
    let initialization_reads = Arc::new(AtomicUsize::new(0));
    let counts = Arc::clone(&initialization_reads);
    let reads = Arc::new(Mutex::new(HashMap::new()));
    let seen = Arc::clone(&reads);
    let task = std::thread::spawn(move || {
      let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
      runtime.block_on(async move {
        loop {
          tokio::select! {
            biased;
            _ = stopping.wait_for(|stopped| *stopped) => break,
            request = receiver.recv() => {
              let Some(request) = request else { break };
              match request {
                ReadRequest::StateValue { key, reply } => {
                  *seen.lock().unwrap().entry(key).or_default() += 1;
                  let _ = reply.send(Ok(values.get(&key).cloned().unwrap_or(None)));
                }
                ReadRequest::Balance { address, reply } => {
                  let value = values.get(&StateKey::Account(address))
                    .and_then(|entry| entry.as_ref()).map_or(U256::zero(), |entry| entry.account().balance);
                  let _ = reply.send(Ok(value));
                }
                ReadRequest::StoragePointsInitialization { address, reply } => {
                  let (expected, result) = initialization.as_ref()
                    .expect("unexpected storage-point initialization read");
                  assert_eq!(&address, expected, "initialization read must target the configured account");
                  counts.fetch_add(1, Ordering::SeqCst);
                  let _ = reply.send(result.clone());
                }
                _ => panic!("unexpected history request in a state test"),
              }
            }
          }
        }
      });
    });
    Self {
      client,
      initialization_reads,
      reads,
      stop,
      task: Some(task),
    }
  }

  pub(crate) fn state_reads(&self, key: StorageKeyWithSpace<'_>) -> usize {
    self
      .reads
      .lock()
      .unwrap()
      .get(&StateKey::try_from(key).unwrap())
      .copied()
      .unwrap_or(0)
  }
}

impl Drop for MockFork {
  fn drop(&mut self) {
    self.stop.send_replace(true);
    if let Some(task) = self.task.take() {
      let result = task.join();
      if !std::thread::panicking() {
        result.expect("mock fork worker panicked");
      }
    }
  }
}
