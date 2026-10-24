//! Stable eSpace projections of local epoch artifacts.

mod phantom;

use std::sync::Arc;

use cfx_executor::machine::Machine;
use cfx_types::{AddressSpaceUtil, Bloom, H256, Space, U256};
use primitives::{
  BlockHeader, BlockNumber, LogEntry, Receipt, SignedTransaction, TransactionStatus,
  block::BlockHeight, transaction::eth_transaction::Eip155Transaction,
};

use crate::runtime_transaction::RuntimeTransaction;

use super::{
  BlockIndex, EpochArtifacts, EpochBlock, EpochTransactionIndex, EpochView, MinedBlockView,
  MinedTransactionView,
};
use phantom::{PhantomTransaction, recover_phantom};

struct PhantomExecution {
  transaction: SignedTransaction,
  receipt: Receipt,
}

impl PhantomExecution {
  fn from_recovered(
    phantom: PhantomTransaction,
    chain_id: u32,
    accumulated_gas_used: U256,
  ) -> Self {
    let PhantomTransaction {
      sender,
      nonce,
      action,
      value,
      data,
      log_bloom,
      logs,
      outcome_status,
    } = phantom;
    let transaction = Eip155Transaction {
      nonce,
      action,
      value,
      data,
      chain_id: Some(chain_id),
      gas: U256::zero(),
      gas_price: U256::zero(),
    }
    .fake_sign_phantom(sender.with_space(Space::Ethereum));

    Self {
      transaction,
      receipt: Receipt {
        accumulated_gas_used,
        log_bloom,
        logs,
        outcome_status,
        ..Default::default()
      },
    }
  }
}

enum ProjectedTransaction {
  Original {
    source: EpochTransactionIndex,
    accumulated_gas_used: U256,
    first_log_index: usize,
  },
  Phantom {
    source: EpochTransactionIndex,
    index_in_source: usize,
    execution: Box<PhantomExecution>,
    first_log_index: usize,
  },
}

impl ProjectedTransaction {
  fn source(&self) -> EpochTransactionIndex {
    match self {
      Self::Original { source, .. } | Self::Phantom { source, .. } => *source,
    }
  }

  fn transaction<'a>(&'a self, artifacts: &'a EpochArtifacts) -> EspaceTransactionRef<'a> {
    match self {
      Self::Original { source, .. } => {
        EspaceTransactionRef::Original(artifacts.transaction(*source))
      }
      Self::Phantom { execution, .. } => EspaceTransactionRef::Phantom(&execution.transaction),
    }
  }

  fn receipt<'a>(&'a self, artifacts: &'a EpochArtifacts) -> EspaceReceiptRef<'a> {
    match self {
      Self::Original {
        source,
        accumulated_gas_used,
        ..
      } => {
        let result = artifacts.transaction_receipt(*source);
        EspaceReceiptRef {
          receipt: result.receipt(),
          accumulated_gas_used: *accumulated_gas_used,
          execution_error_message: result.execution_error_message(),
        }
      }
      Self::Phantom { execution, .. } => EspaceReceiptRef {
        receipt: &execution.receipt,
        accumulated_gas_used: execution.receipt.accumulated_gas_used,
        // Cross-space events provide an outcome, but no VM error text.
        execution_error_message: None,
      },
    }
  }

  fn phantom_index(&self) -> Option<usize> {
    match self {
      Self::Original { .. } => None,
      Self::Phantom {
        index_in_source, ..
      } => Some(*index_in_source),
    }
  }

  fn hash(&self, artifacts: &EpochArtifacts) -> H256 {
    self.transaction(artifacts).hash()
  }

  fn accumulated_gas_used(&self) -> U256 {
    match self {
      Self::Original {
        accumulated_gas_used,
        ..
      } => *accumulated_gas_used,
      Self::Phantom { execution, .. } => execution.receipt.accumulated_gas_used,
    }
  }

  fn first_log_index(&self) -> usize {
    match self {
      Self::Original {
        first_log_index, ..
      }
      | Self::Phantom {
        first_log_index, ..
      } => *first_log_index,
    }
  }
}

/// Ordinary transactions borrow their epoch's artifacts; only phantom entries own
/// derived transactions and receipts. Genesis has an empty eSpace projection.
#[derive(Default)]
pub(super) struct EspaceProjection {
  transactions: Vec<ProjectedTransaction>,
  bloom: Bloom,
  total_gas_limit: U256,
}

impl EspaceProjection {
  /// Borrows transaction receipts whose block owner has established correspondence.
  pub(super) fn from_executed_epoch(
    machine: &Machine,
    start_block_number: BlockNumber,
    epoch_height: BlockHeight,
    blocks: &[EpochBlock],
  ) -> Self {
    let params = machine.params();
    let chain_id = params
      .chain_id
      .read()
      .get_chain_id(epoch_height)
      .in_evm_space();
    let mut projection = Self::default();
    let mut accumulated_gas_used = U256::zero();
    let mut first_log_index = 0;

    for (block_index, epoch_block) in blocks.iter().enumerate() {
      let block = epoch_block.runtime_block();
      let block_number = start_block_number
        .checked_add(block_index as u64)
        .expect("an executed epoch must fit the Core block number range");
      let spec = machine.spec(block_number, epoch_height);
      projection.total_gas_limit += block
        .header()
        .espace_gas_limit(params.can_pack_evm_transaction(block.header().height()));

      // Upstream carries the last projected cumulative value between blocks.
      // A normal receipt's value includes both Spaces in its original block.
      let gas_used_offset = accumulated_gas_used;
      for result in epoch_block.transaction_receipts() {
        let transaction = result.transaction();
        let receipt = result.receipt();
        let source = EpochTransactionIndex {
          block_index,
          transaction_index: result.block_transaction_index(),
        };
        match transaction.space() {
          Space::Ethereum if !receipt.tx_skipped() => {
            accumulated_gas_used = gas_used_offset + receipt.accumulated_gas_used;
            projection.bloom.accrue_bloom(&receipt.log_bloom);
            projection
              .transactions
              .push(ProjectedTransaction::Original {
                source,
                accumulated_gas_used,
                first_log_index,
              });
            first_log_index += receipt.logs.len();
          }
          Space::Native if receipt.tx_success() && spec.cip90 => {
            for (index_in_source, phantom) in recover_phantom(&receipt.logs, transaction.hash())
              .into_iter()
              .enumerate()
            {
              let execution =
                PhantomExecution::from_recovered(phantom, chain_id, accumulated_gas_used);
              projection.bloom.accrue_bloom(&execution.receipt.log_bloom);
              let log_count = execution.receipt.logs.len();
              projection.transactions.push(ProjectedTransaction::Phantom {
                source,
                index_in_source,
                execution: Box::new(execution),
                first_log_index,
              });
              first_log_index += log_count;
            }
          }
          Space::Native | Space::Ethereum => {}
        }
      }
    }

    projection
  }

  pub(super) fn transactions<'a>(
    &'a self,
    artifacts: &'a EpochArtifacts,
  ) -> impl Iterator<Item = EspaceTransactionRef<'a>> + 'a {
    self
      .transactions
      .iter()
      .map(move |transaction| transaction.transaction(artifacts))
  }
}

/// Pins one local epoch's eSpace block, independently of later history changes.
/// This view does not imply that the epoch is visible to a particular RPC query.
#[derive(Clone)]
pub(crate) struct EspaceBlockView {
  epoch_view: Arc<EpochView>,
}

impl EspaceBlockView {
  pub(super) fn new(epoch_view: Arc<EpochView>) -> Option<Self> {
    epoch_view.artifacts()?;
    Some(Self { epoch_view })
  }

  fn artifacts(&self) -> &EpochArtifacts {
    self.epoch_view.local_artifacts()
  }

  pub(crate) fn hash(&self) -> H256 {
    self.epoch_view.state().epoch_id
  }

  /// An eSpace block number is its epoch height, not a Core block number.
  pub(crate) fn epoch_height(&self) -> BlockHeight {
    self.epoch_view.epoch_height()
  }

  pub(crate) fn pivot_header(&self) -> &BlockHeader {
    self.artifacts().pivot_runtime_block().header()
  }

  pub(crate) fn total_gas_limit(&self) -> U256 {
    self.artifacts().espace.total_gas_limit
  }

  pub(crate) fn gas_used(&self) -> U256 {
    self
      .artifacts()
      .espace
      .transactions
      .last()
      .map_or(U256::zero(), ProjectedTransaction::accumulated_gas_used)
  }

  pub(crate) fn logs_bloom(&self) -> &Bloom {
    &self.artifacts().espace.bloom
  }

  pub(crate) fn transaction_at(&self, transaction_index: usize) -> Option<EspaceTransactionView> {
    self
      .artifacts()
      .espace
      .transactions
      .get(transaction_index)?;
    Some(EspaceTransactionView {
      block: self.clone(),
      transaction_index,
    })
  }

  pub(crate) fn transactions(&self) -> impl ExactSizeIterator<Item = EspaceTransactionView> + '_ {
    (0..self.artifacts().espace.transactions.len()).map(move |transaction_index| {
      EspaceTransactionView {
        block: self.clone(),
        transaction_index,
      }
    })
  }
}

/// Phantom signatures encode query identity and never establish pool admission.
#[derive(Clone, Copy)]
pub(crate) enum EspaceTransactionRef<'a> {
  Original(&'a RuntimeTransaction),
  Phantom(&'a SignedTransaction),
}

impl EspaceTransactionRef<'_> {
  pub(crate) fn hash(&self) -> H256 {
    match self {
      Self::Original(transaction) => transaction.hash(),
      Self::Phantom(transaction) => transaction.hash(),
    }
  }

  pub(crate) fn sender(&self) -> cfx_types::Address {
    match self {
      Self::Original(transaction) => transaction.sender(),
      Self::Phantom(transaction) => transaction.sender,
    }
  }

  pub(crate) fn transaction(&self) -> &primitives::Transaction {
    match self {
      Self::Original(transaction) => transaction.transaction(),
      Self::Phantom(transaction) => &transaction.unsigned,
    }
  }
}

/// Borrows eSpace receipt fields while keeping cumulative gas in projected order.
/// The raw receipt stays private so its block-local cumulative value cannot leak.
#[derive(Clone, Copy)]
pub(crate) struct EspaceReceiptRef<'a> {
  receipt: &'a Receipt,
  accumulated_gas_used: U256,
  execution_error_message: Option<&'a str>,
}

impl<'a> EspaceReceiptRef<'a> {
  pub(crate) fn accumulated_gas_used(&self) -> U256 {
    self.accumulated_gas_used
  }

  pub(crate) fn gas_fee(&self) -> U256 {
    self.receipt.gas_fee
  }

  pub(crate) fn burnt_gas_fee(&self) -> Option<U256> {
    self.receipt.burnt_gas_fee
  }

  pub(crate) fn outcome_status(&self) -> TransactionStatus {
    self.receipt.outcome_status
  }

  pub(crate) fn logs(&self) -> &'a [LogEntry] {
    &self.receipt.logs
  }

  pub(crate) fn logs_bloom(&self) -> &'a Bloom {
    &self.receipt.log_bloom
  }

  pub(crate) fn execution_error_message(&self) -> Option<&'a str> {
    self.execution_error_message
  }
}

#[derive(Clone)]
pub(crate) struct EspaceTransactionView {
  block: EspaceBlockView,
  transaction_index: usize,
}

impl EspaceTransactionView {
  fn projected(&self) -> &ProjectedTransaction {
    &self.block.artifacts().espace.transactions[self.transaction_index]
  }

  pub(crate) fn block(&self) -> &EspaceBlockView {
    &self.block
  }

  /// Position in the entire epoch's eSpace sequence, including phantom entries.
  pub(crate) fn transaction_index(&self) -> usize {
    self.transaction_index
  }

  pub(crate) fn hash(&self) -> H256 {
    self.projected().hash(self.block.artifacts())
  }

  pub(crate) fn transaction(&self) -> EspaceTransactionRef<'_> {
    self.projected().transaction(self.block.artifacts())
  }

  pub(crate) fn receipt(&self) -> EspaceReceiptRef<'_> {
    self.projected().receipt(self.block.artifacts())
  }

  /// Gas attributed to this entry by adjacent projected cumulative values.
  pub(crate) fn gas_used(&self) -> U256 {
    let previous_gas = self
      .transaction_index
      .checked_sub(1)
      .map_or(U256::zero(), |index| {
        self.block.artifacts().espace.transactions[index].accumulated_gas_used()
      });
    self.projected().accumulated_gas_used() - previous_gas
  }

  /// The first log's index in the entire epoch's eSpace sequence.
  pub(crate) fn first_log_index(&self) -> usize {
    self.projected().first_log_index()
  }

  /// Ordinal among phantom transactions recovered from the same Native transaction.
  pub(crate) fn phantom_index(&self) -> Option<usize> {
    self.projected().phantom_index()
  }

  /// The original block transaction; for a phantom entry this is its Native source.
  pub(crate) fn source_transaction(&self) -> MinedTransactionView {
    let source = self.projected().source();
    MinedTransactionView {
      block: MinedBlockView {
        epoch_view: Arc::clone(&self.block.epoch_view),
        block_index: BlockIndex::Ordered(source.block_index),
      },
      transaction_index: source.transaction_index,
    }
  }
}
