//! A temporary sponsorship change distinguishes otherwise-hidden CIP-107 state.

use cfx_parameters::{
  consensus::ONE_CFX_IN_DRIP,
  internal_contract_addresses::{
    CONTEXT_CONTRACT_ADDRESS, SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS,
  },
};
use cfx_rpc_cfx_types::TransactionRequest;
use cfx_types::{Address, U256, U512};
use keccak_hash::keccak;
use xcfx_state::account::Account;

use super::{ForkBase, ForkReadError};

pub(super) struct InitializationProbe {
  request: TransactionRequest,
  uninitialized_balance: U256,
  initialized_balance: U256,
}

impl InitializationProbe {
  pub(super) fn new(
    base: &ForkBase,
    account: &Account,
    prop: U256,
    sponsor_contract_balance: U256,
  ) -> Result<Self, ForkReadError> {
    if prop.is_zero() {
      return Err(ForkReadError::Unavailable(
        "storage-point initialization at a zero conversion ratio",
      ));
    }
    let denominator = U256::from(ONE_CFX_IN_DRIP)
      .checked_add(prop)
      .ok_or(ForkReadError::OutOfRange("storage-point conversion ratio"))?;
    let balance = account.sponsor_info.sponsor_balance_for_collateral;
    let collateral = account.collateral_for_storage;

    // ceil((D + R) / R) guarantees at least one converted drip. Include
    // the builtin's existing balance, which its setter consumes as well.
    let ratio = U512::from(prop);
    let increment = (U512::from(denominator) + ratio - U512::one()) / ratio;
    let required = U512::from(balance) + U512::from(collateral) + increment;
    let total = U256::try_from(required.max(U512::from(sponsor_contract_balance)))
      .map_err(|_| ForkReadError::OutOfRange("initialization probe funding"))?;
    let value = total - sponsor_contract_balance;
    let uninitialized_balance = total - collateral;
    let increase = uninitialized_balance - balance;
    // Match the upstream setter's U256 intermediates, not just its final quotient.
    let converted = increase
      .checked_mul(prop)
      .ok_or(ForkReadError::OutOfRange("initialization probe conversion"))?
      / denominator;
    let initialized_balance = uninitialized_balance - converted;

    let target = address_word(account.address().address);
    let sponsor = SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS;
    let mut code = ProbeCode::default();
    code.query(CONTEXT_CONTRACT_ADDRESS, "epochNumber()", &[]);
    code.require_word(U256::from(base.epoch_height) + U256::one(), b"EPOCH");
    code.query(
      CONTEXT_CONTRACT_ADDRESS,
      "epochHash(uint256)",
      &[U256::from(base.epoch_height)],
    );
    code.require_word(U256::from_big_endian(base.pivot_hash.as_bytes()), b"PIVOT");
    code.query(sponsor, "getSponsorForCollateral(address)", &[target]);
    code.require_word(
      address_word(account.sponsor_info.sponsor_for_collateral),
      b"SPONSOR",
    );
    code.op(0x30); // ADDRESS
    code.push(address_word(account.sponsor_info.sponsor_for_collateral));
    code.op(0x14); // EQ
    code.op(0x15); // ISZERO: require the replacement-sponsor branch.
    code.require(b"CALLER");
    code.query(
      sponsor,
      "getSponsoredBalanceForCollateral(address)",
      &[target],
    );
    code.require_word(balance, b"BALANCE");
    code.push(address_word(sponsor));
    code.op(0x31); // BALANCE
    code.require_word(sponsor_contract_balance, b"FUNDS");
    code.pay(sponsor, "setSponsorForCollateral(address)", &[target]);
    code.query(
      sponsor,
      "getSponsoredBalanceForCollateral(address)",
      &[target],
    );

    // A zero first word prevents the returned creation code from starting
    // with the forbidden 0xef prefix, regardless of the observed balance.
    code.push(U256::from(32));
    code.op(0x52); // MSTORE: observed balance.
    code.store(0, U256::zero());
    code.push(U256::from(64));
    code.push(U256::zero());
    code.op(0xf3); // RETURN

    Ok(Self {
      // cfx_call funds an omitted sender in its private state. Leave gas,
      // gas price and storage limit to the remote call's normal defaults.
      request: TransactionRequest {
        value: Some(value),
        data: Some(code.0.into()),
        ..Default::default()
      },
      uninitialized_balance,
      initialized_balance,
    })
  }

  pub(super) fn request(&self) -> &TransactionRequest {
    &self.request
  }

  pub(super) fn decode(&self, output: &[u8]) -> Result<bool, ForkReadError> {
    if output.len() != 64 || output[..32].iter().any(|byte| *byte != 0) {
      return Err(ForkReadError::Inconsistent("initialization probe output"));
    }
    let balance = U256::from_big_endian(&output[32..]);
    if balance == self.uninitialized_balance {
      Ok(false)
    } else if balance == self.initialized_balance {
      Ok(true)
    } else {
      Err(ForkReadError::Inconsistent("initialization probe balance"))
    }
  }
}

pub(super) fn address_word(address: Address) -> U256 {
  U256::from_big_endian(address.as_bytes())
}

/// Encodes the fixed-width arguments used by these builtin queries.
pub(super) fn calldata(signature: &str, arguments: &[U256]) -> Vec<u8> {
  let mut data = Vec::with_capacity(4 + arguments.len() * 32);
  data.extend_from_slice(&keccak(signature.as_bytes()).as_bytes()[..4]);
  for argument in arguments {
    data.extend_from_slice(&argument.to_big_endian());
  }
  data
}

// Only emits the fixed constructor above; this is not a general VM assembler.
#[derive(Default)]
struct ProbeCode(Vec<u8>);

impl ProbeCode {
  fn op(&mut self, opcode: u8) {
    self.0.push(opcode);
  }

  fn push(&mut self, value: U256) {
    let bytes = value.to_big_endian();
    let start = bytes.iter().position(|byte| *byte != 0).unwrap_or(31);
    self.op(0x5f + (32 - start) as u8);
    self.0.extend_from_slice(&bytes[start..]);
  }

  fn store(&mut self, offset: usize, value: U256) {
    self.push(value);
    self.push(offset.into());
    self.op(0x52); // MSTORE
  }

  fn input(&mut self, signature: &str, arguments: &[U256]) -> usize {
    let data = calldata(signature, arguments);
    for (index, chunk) in data.chunks(32).enumerate() {
      let mut word = [0; 32];
      word[..chunk.len()].copy_from_slice(chunk);
      self.store(index * 32, U256::from_big_endian(&word));
    }
    data.len()
  }

  fn query(&mut self, address: Address, signature: &str, arguments: &[U256]) {
    let length = self.input(signature, arguments);
    self.push(32.into());
    self.push(128.into());
    self.push(length.into());
    self.push(U256::zero());
    self.push(address_word(address));
    self.op(0x5a); // GAS
    self.op(0xfa); // STATICCALL
    self.require(b"QUERY");
    self.op(0x3d); // RETURNDATASIZE
    self.require_word(32.into(), b"RETURN");
    self.push(128.into());
    self.op(0x51); // MLOAD
  }

  fn pay(&mut self, address: Address, signature: &str, arguments: &[U256]) {
    let length = self.input(signature, arguments);
    self.push(U256::zero());
    self.push(U256::zero());
    self.push(length.into());
    self.push(U256::zero());
    self.op(0x34); // CALLVALUE
    self.push(address_word(address));
    self.op(0x5a); // GAS
    self.op(0xf1); // CALL
    self.require(b"SPONSOR_SET");
  }

  fn require_word(&mut self, expected: U256, reason: &[u8]) {
    self.push(expected);
    self.op(0x14); // EQ
    self.require(reason);
  }

  fn require(&mut self, reason: &[u8]) {
    self.op(0x61); // PUSH2: jump past the reverting branch on success.
    let offset = self.0.len();
    self.0.extend_from_slice(&[0, 0]);
    self.op(0x57); // JUMPI
    let mut message = [0; 32];
    message[..reason.len()].copy_from_slice(reason);
    self.store(0, U256::from_big_endian(&message));
    self.push(reason.len().into());
    self.push(U256::zero());
    self.op(0xfd); // REVERT
    let target = u16::try_from(self.0.len()).expect("the fixed probe fits a PUSH2 jump");
    self.0[offset..offset + 2].copy_from_slice(&target.to_be_bytes());
    self.op(0x5b); // JUMPDEST
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::{
    chain_spec::ChainSpec,
    fork::tests::base,
    state::execution_tests::{initialized_parent, reference},
  };
  use cfx_addr::Network;
  use cfx_executor::{
    executive::{
      ChargeCollateral, ExecutionError, ExecutionOutcome, ExecutiveContext, TransactOptions,
      TransactSettings,
    },
    internal_contract::{epoch_hash_slot, storage_point_prop},
  };
  use cfx_rpc_cfx_types::RpcAddress;
  use cfx_types::{AddressSpaceUtil, H256};
  use cfx_vm_types::{Env, Error as VmError};
  use primitives::storage::STORAGE_LAYOUT_REGULAR_V0;
  use xcfx_state::account::StoragePointsState;

  struct ProbeFixture {
    balance: u64,
    collateral: u64,
    builtin_balance: u64,
    ratio: U256,
  }

  impl ProbeFixture {
    fn account(&self) -> Account {
      let mut account = Account::new_empty(&Address::from([0x80; 20]).with_native_space());
      account.code_hash = keccak([0x00]);
      account.collateral_for_storage = self.collateral.into();
      account.sponsor_info.sponsor_for_collateral = Address::from([0x11; 20]);
      account.sponsor_info.sponsor_balance_for_collateral = self.balance.into();
      account.sponsor_info.storage_points = StoragePointsState::Unknown;
      account
    }

    fn probe(&self) -> InitializationProbe {
      InitializationProbe::new(
        &base(),
        &self.account(),
        self.ratio,
        self.builtin_balance.into(),
      )
      .unwrap()
    }

    fn execute(
      &self,
      probe: &InitializationProbe,
      initialized: bool,
      epoch_height: u64,
      pivot_hash: H256,
    ) -> ExecutionOutcome {
      let base = base();
      let chain = ChainSpec::new(base.network.chain_ids);
      let parent = initialized_parent(&chain);
      let root_before = parent.root_with_aux_info();
      let (mut state, committed) = reference(&parent);
      let account = self.account();
      let target = account.address();
      let sponsor = account.sponsor_info.sponsor_for_collateral;
      let sender = Address::from([0x12; 20]);
      let funds = U256::from(ONE_CFX_IN_DRIP) * U256::from(100);
      state.add_total_issued(funds);
      state
        .add_balance(&sender.with_native_space(), &funds)
        .unwrap();
      state
        .new_contract_with_admin(
          target,
          &sponsor,
          U256::zero(),
          Some(STORAGE_LAYOUT_REGULAR_V0),
          initialized,
        )
        .unwrap();
      state
        .init_code(target, vec![0x00], sponsor, H256::zero())
        .unwrap();
      state
        .set_sponsor_for_collateral(
          &target.address,
          &sponsor,
          &(U256::from(self.balance) + U256::from(self.collateral)),
          false,
        )
        .unwrap();
      state
        .add_collateral_for_storage(&target.address, &self.collateral.into())
        .unwrap();
      state
        .add_balance(
          &SPONSOR_WHITELIST_CONTROL_CONTRACT_ADDRESS.with_native_space(),
          &self.builtin_balance.into(),
        )
        .unwrap();
      state
        .set_system_storage(storage_point_prop().to_vec(), self.ratio)
        .unwrap();
      state
        .set_system_storage(
          epoch_hash_slot(base.epoch_height).to_vec(),
          U256::from_big_endian(pivot_hash.as_bytes()),
        )
        .unwrap();
      let saved = state.save();
      let sponsor_before = state.sponsor_info(&target.address).unwrap();
      // Reading after save populates the transaction cache again.
      state.commit_cache(false);
      let mut request = probe.request().clone();
      request.from = Some(
        RpcAddress::try_from_h160(sender, Network::Id(base.network.chain_ids.network_id)).unwrap(),
      );
      let gas = U256::from(2_000_000);
      request.gas = Some(gas);
      let tx = request
        .sign_call(
          base.epoch_height,
          base.network.chain_ids.chain_id,
          Some(gas),
        )
        .unwrap();
      let env = Env {
        number: base.pivot_block_number + 1,
        epoch_height,
        chain_id: chain.machine().params().chain_id_map(epoch_height),
        gas_limit: gas,
        transaction_hash: tx.hash(),
        ..Default::default()
      };
      let spec = chain.machine().spec(env.number, epoch_height);
      let outcome = ExecutiveContext::new(&mut state, &env, chain.machine(), &spec)
        .transact(
          &tx,
          TransactOptions {
            observer: (),
            settings: TransactSettings {
              charge_collateral: ChargeCollateral::EstimateSender,
              charge_gas: false,
              check_base_price: false,
              check_epoch_bound: false,
              ..TransactSettings::all_checks()
            },
          },
        )
        .unwrap();
      state.restore(saved);
      assert_eq!(state.sponsor_info(&target.address).unwrap(), sponsor_before);
      assert_eq!(
        state.token_collateral_for_storage(&target.address).unwrap(),
        self.collateral.into()
      );
      assert_eq!(state.total_issued_tokens(), funds);
      assert!(committed.lock().unwrap().is_none());
      assert_eq!(parent.root_with_aux_info(), root_before);
      outcome
    }
  }

  #[test]
  fn probe_distinguishes_known_flags_in_the_conflux_vm() {
    let unit = U256::from(ONE_CFX_IN_DRIP);
    // Small drips exercise rounding; the third and last cases spend the
    // builtin's existing balance without sending any additional value.
    for (balance, collateral, builtin_balance, ratio) in [
      (0, 0, 0, unit),
      (7, 11, 0, unit),
      (7, 11, 100, unit),
      (4, 3, 0, unit / 2),
      (0, 0, 0, U256::one()),
      (4, 1, 50, unit * 2),
    ] {
      let fixture = ProbeFixture {
        balance,
        collateral,
        builtin_balance,
        ratio,
      };
      let probe = fixture.probe();
      for initialized in [false, true] {
        let result = fixture.execute(
          &probe,
          initialized,
          base().epoch_height + 1,
          base().pivot_hash,
        );
        let ExecutionOutcome::Finished(executed) = result else {
          panic!(
            "probe did not finish for initialized={initialized}, B={balance}, C={collateral}, P={builtin_balance}, R={ratio}: {result:?}"
          );
        };
        assert_eq!(probe.decode(&executed.output).unwrap(), initialized);
      }
    }
  }

  #[test]
  fn probe_reverts_when_the_simulated_epoch_or_pivot_is_wrong() {
    let fixture = ProbeFixture {
      balance: 7,
      collateral: 11,
      builtin_balance: 0,
      ratio: ONE_CFX_IN_DRIP.into(),
    };
    let probe = fixture.probe();
    for (epoch, pivot, reason) in [
      (
        base().epoch_height + 2,
        base().pivot_hash,
        b"EPOCH".as_slice(),
      ),
      (base().epoch_height + 1, H256::zero(), b"PIVOT".as_slice()),
    ] {
      let result = fixture.execute(&probe, false, epoch, pivot);
      let ExecutionOutcome::ExecutionErrorBumpNonce(
        ExecutionError::VmError(VmError::Reverted),
        executed,
      ) = result
      else {
        panic!("a mismatched base must revert: {result:?}");
      };
      assert_eq!(executed.output, reason);
    }
  }

  #[test]
  fn ambiguous_overflowing_and_malformed_probe_results_are_errors() {
    let fixture = ProbeFixture {
      balance: 0,
      collateral: 0,
      builtin_balance: 0,
      ratio: ONE_CFX_IN_DRIP.into(),
    };
    let mut account = fixture.account();
    assert!(matches!(
      InitializationProbe::new(&base(), &account, U256::zero(), U256::zero()),
      Err(ForkReadError::Unavailable(_))
    ));
    assert!(matches!(
      InitializationProbe::new(&base(), &account, U256::MAX, U256::zero()),
      Err(ForkReadError::OutOfRange(_))
    ));
    assert!(matches!(
      InitializationProbe::new(&base(), &account, fixture.ratio, U256::MAX),
      Err(ForkReadError::OutOfRange(_))
    ));
    account.sponsor_info.sponsor_balance_for_collateral = U256::MAX;
    assert!(matches!(
      InitializationProbe::new(&base(), &account, fixture.ratio, U256::zero()),
      Err(ForkReadError::OutOfRange(_))
    ));

    let probe = fixture.probe();
    for bytes in [vec![], vec![0; 32], vec![0; 64], vec![1; 64], vec![0; 96]] {
      assert!(probe.decode(&bytes).is_err());
    }
  }
}
