use std::collections::{BTreeMap, BTreeSet};

use cfx_executor::observer::{OpcodeTracer, SetAuthTracer};
use xcfx_state::State;

use cfx_parity_trace_types::{SetAuth, SetAuthOutcome};
use cfx_types::{Address, AddressSpaceUtil, H256, u256_to_address_be, u256_to_h256_be};
use cfx_vm_interpreter::instructions::Instruction;
use cfx_vm_types::InterpreterInfo;
use primitives::{AccessList, AccessListItem, extract_7702_payload};

#[derive(Debug)]
pub(in crate::virtual_execution) struct AccessListObserver {
  excluded: BTreeSet<Address>,
  touched: BTreeMap<Address, BTreeSet<H256>>,
  call_targets: BTreeSet<Address>,
  delegation_updates: BTreeMap<Address, Option<Address>>,
}

impl AccessListObserver {
  pub(in crate::virtual_execution) fn new(seed: &AccessList, excluded: BTreeSet<Address>) -> Self {
    let mut touched = BTreeMap::<Address, BTreeSet<H256>>::new();
    for item in seed {
      touched
        .entry(item.address)
        .or_default()
        .extend(item.storage_keys.iter().copied());
    }
    Self {
      excluded,
      touched,
      call_targets: BTreeSet::new(),
      delegation_updates: BTreeMap::new(),
    }
  }

  /// Resolves implicit code accesses against the restored, prepared state.
  pub(in crate::virtual_execution) fn into_access_list(
    mut self,
    prepared_state: &State<'_>,
  ) -> cfx_statedb::Result<AccessList> {
    for address in std::mem::take(&mut self.call_targets) {
      let delegation = match self.delegation_updates.get(&address) {
        // A successful clear overrides any delegation in the prepared state.
        Some(delegation) => *delegation,
        None => prepared_state
          .code(&address.with_evm_space())?
          .as_deref()
          .and_then(|code| extract_7702_payload(code)),
      };
      if let Some(delegation) = delegation {
        self.touch_account(delegation);
      }
    }
    Ok(
      self
        .touched
        .into_iter()
        .map(|(address, keys)| AccessListItem {
          address,
          storage_keys: keys.into_iter().collect(),
        })
        .collect(),
    )
  }

  fn touch_account(&mut self, address: Address) {
    if !self.excluded.contains(&address) {
      self.touched.entry(address).or_default();
    }
  }
}

impl OpcodeTracer for AccessListObserver {
  fn step(&mut self, interp: &dyn InterpreterInfo) {
    let Some(instruction) = Instruction::from_u8(interp.current_opcode()) else {
      return;
    };
    let stack = interp.stack();
    use Instruction::*;
    match instruction {
      SLOAD | SSTORE => {
        let required = if instruction == SSTORE { 2 } else { 1 };
        if stack.len() >= required {
          self
            .touched
            .entry(interp.contract_address())
            .or_default()
            .insert(u256_to_h256_be(stack[stack.len() - 1]));
        }
      }
      BALANCE | EXTCODESIZE | EXTCODEHASH | SUICIDE | EXTCODECOPY => {
        let required = if instruction == EXTCODECOPY { 4 } else { 1 };
        if stack.len() >= required {
          self.touch_account(u256_to_address_be(stack[stack.len() - 1]));
        }
      }
      CALL | CALLCODE | DELEGATECALL | STATICCALL => {
        let required = if matches!(instruction, CALL | CALLCODE) {
          7
        } else {
          6
        };
        if stack.len() >= required {
          // The top argument is gas; the destination is immediately below it.
          let address = u256_to_address_be(stack[stack.len() - 2]);
          self.touch_account(address);
          // A prewarmed authority can still delegate to a cold code address.
          self.call_targets.insert(address);
        }
      }
      _ => {}
    }
  }
}

impl SetAuthTracer for AccessListObserver {
  fn record_set_auth(&mut self, action: SetAuth) {
    let Some(authority) = action.author else {
      return;
    };
    // A recovered authority has been warmed even if its code or nonce fails validation.
    self.excluded.insert(authority);
    if action.outcome == SetAuthOutcome::Success {
      self.delegation_updates.insert(
        authority,
        (!action.address.is_zero()).then_some(action.address),
      );
    }
  }
}
