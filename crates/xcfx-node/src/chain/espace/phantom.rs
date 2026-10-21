//! Reconstructs query-only transactions from local cross-space execution logs.
//!
//! Follows Conflux `execute-helper/src/phantom_tx` at revision 4dc2968d0ba1556e8a77ba1e6f0b0f703e7533c6.
//! Event definitions and ABI encoding use upstream primitives.

use cfx_executor::internal_contract::{
  SolidityEventTrait,
  cross_space_events::{CallEvent, CreateEvent, ReturnEvent, WithdrawEvent},
};
use cfx_parameters::internal_contract_addresses::CROSS_SPACE_CONTRACT_ADDRESS;
use cfx_types::{Address, Bloom, H256, Space, U256};
use primitives::{Action, LogEntry, TransactionStatus};
use solidity_abi::{ABIDecodable, ABIEncodable};

/// Recovered execution facts, before chain identity and epoch accounting are assigned.
pub(super) struct PhantomTransaction {
  pub(super) sender: Address,
  pub(super) nonce: U256,
  pub(super) action: Action,
  pub(super) value: U256,
  pub(super) data: Vec<u8>,
  pub(super) log_bloom: Bloom,
  pub(super) logs: Vec<LogEntry>,
  pub(super) outcome_status: TransactionStatus,
}

impl PhantomTransaction {
  fn transfer(sender: Address, to: Address, nonce: U256, value: U256, data: Vec<u8>) -> Self {
    Self {
      sender,
      nonce,
      action: Action::Call(to),
      value,
      data,
      log_bloom: Bloom::zero(),
      logs: Vec::new(),
      outcome_status: TransactionStatus::Success,
    }
  }
}

struct CrossSpaceCall {
  sender: Address,
  nonce: U256,
  action: Action,
  value: U256,
  data: Vec<u8>,
  log_bloom: Bloom,
  logs: Vec<LogEntry>,
}

impl CrossSpaceCall {
  fn finish(self, success: bool) -> PhantomTransaction {
    PhantomTransaction {
      sender: self.sender,
      nonce: self.nonce,
      action: self.action,
      value: self.value,
      data: self.data,
      log_bloom: self.log_bloom,
      logs: self.logs,
      outcome_status: if success {
        TransactionStatus::Success
      } else {
        TransactionStatus::Failure
      },
    }
  }
}

fn event_address(log: &LogEntry, index: usize) -> Address {
  let topic = log
    .topics
    .get(index)
    .expect("a cross-space event must contain its indexed address");
  Address::from(
    <[u8; 20]>::abi_decode(topic.as_bytes())
      .expect("a cross-space event address must have the declared ABI encoding"),
  )
}

/// The caller supplies logs from a successful local Native receipt with CIP-90 active.
/// Malformed events violate executor invariants; this is not an external log parser.
pub(super) fn recover_phantom(logs: &[LogEntry], source_hash: H256) -> Vec<PhantomTransaction> {
  let mut transactions = Vec::new();
  let mut current_call: Option<CrossSpaceCall> = None;
  let mut cross_space_nonce = 0u32;

  for log in logs {
    if log.address == CROSS_SPACE_CONTRACT_ADDRESS {
      let signature = log
        .topics
        .first()
        .expect("a cross-space event must contain its signature");

      if signature == &CallEvent::EVENT_SIG || signature == &CreateEvent::EVENT_SIG {
        assert!(
          current_call.is_none(),
          "a cross-space call must finish before the next call begins",
        );
        let sender = event_address(log, 1);
        let recipient = event_address(log, 2);
        let (value, nonce, data): (U256, U256, Vec<u8>) = ABIDecodable::abi_decode(&log.data)
          .expect("a cross-space call event must contain value, nonce, and data");

        // This funding transaction encodes a call ordinal, not the final
        // transaction index: each call produces funding and execution entries.
        transactions.push(PhantomTransaction::transfer(
          Address::zero(),
          sender,
          U256::zero(),
          value,
          (source_hash, U256::from(cross_space_nonce)).abi_encode(),
        ));
        cross_space_nonce = cross_space_nonce
          .checked_add(1)
          .expect("a local receipt's cross-space call count must fit in u32");

        current_call = Some(CrossSpaceCall {
          sender,
          nonce,
          action: if signature == &CreateEvent::EVENT_SIG {
            Action::Create
          } else {
            Action::Call(recipient)
          },
          value,
          data,
          log_bloom: Bloom::zero(),
          logs: Vec::new(),
        });
      } else if signature == &WithdrawEvent::EVENT_SIG {
        let sender = event_address(log, 1);
        let (value, nonce): (U256, U256) = ABIDecodable::abi_decode(&log.data)
          .expect("a cross-space withdrawal event must contain value and nonce");
        transactions.push(PhantomTransaction::transfer(
          sender,
          Address::zero(),
          nonce,
          value,
          Vec::new(),
        ));
      } else if signature == &ReturnEvent::EVENT_SIG {
        let success = bool::abi_decode(&log.data)
          .expect("a cross-space return event must contain its success flag");
        let call = current_call
          .take()
          .expect("a cross-space return event must finish an active call");
        transactions.push(call.finish(success));
      }
    } else if log.space == Space::Ethereum {
      if let Some(call) = &mut current_call {
        call.log_bloom.accrue_bloom(&log.bloom());
        call.logs.push(log.clone());
      }
    }
  }

  assert!(
    current_call.is_none(),
    "a successful Native receipt must finish every cross-space call",
  );
  transactions
}
