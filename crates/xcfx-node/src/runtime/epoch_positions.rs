//! Confirmed, finalized, and checkpoint epochs derived from the initial chain view.

use std::num::NonZeroU64;

use cfx_parameters::consensus::ERA_DEFAULT_EPOCH_COUNT;
use primitives::block::BlockHeight;

use crate::chain::EpochView;

const LOCAL_ERA_EPOCH_COUNT: NonZeroU64 =
  NonZeroU64::new(ERA_DEFAULT_EPOCH_COUNT).expect("the local era length must be nonzero");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EpochPositionSource {
  Genesis,
  Fork,
  /// Derived from the instance's deterministic depth and era rules.
  Local,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EpochPosition {
  pub(crate) height: BlockHeight,
  pub(crate) source: EpochPositionSource,
}

/// Positions used by epoch selectors and the reorg boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EpochPositions {
  pub(crate) confirmed: EpochPosition,
  pub(crate) finalized: EpochPosition,
  pub(crate) checkpoint: EpochPosition,
}

impl EpochPosition {
  fn advance_to(self, height: BlockHeight) -> Self {
    if height > self.height {
      Self {
        height,
        source: EpochPositionSource::Local,
      }
    } else {
      self
    }
  }
}

impl EpochPositions {
  pub(crate) fn reorg_boundary(&self) -> BlockHeight {
    self.finalized.height.max(self.checkpoint.height)
  }
}

/// Immutable offsets behind `latest_state` for local confirmation and finality.
#[derive(Clone, Copy)]
pub(super) struct ConfirmationDepths {
  pub(super) confirmed: u64,
  pub(super) finalized: u64,
}

/// Computes positions without modifying the chain or its initial view.
///
/// `None` initializes from Genesis or Fork metadata on startup and reset.
/// Otherwise only confirmed may retreat; finalized and checkpoint never retreat.
/// Snapshot revert restores saved positions without deriving them.
pub(super) fn derive_epoch_positions(
  initial_view: &EpochView,
  previous: Option<EpochPositions>,
  latest_state: BlockHeight,
  depths: ConfirmationDepths,
) -> EpochPositions {
  let (baseline, era_epoch_count) = if let Some(base) = initial_view.fork_base() {
    let position = |height| EpochPosition {
      height,
      source: EpochPositionSource::Fork,
    };
    (
      EpochPositions {
        confirmed: position(base.latest_confirmed.min(base.epoch_height)),
        finalized: position(base.latest_finalized.min(base.epoch_height)),
        checkpoint: position(
          base
            .latest_checkpoint
            .min(align_to_era(base.epoch_height, base.era_epoch_count)),
        ),
      },
      base.era_epoch_count,
    )
  } else {
    let genesis = EpochPosition {
      height: initial_view.epoch_height(),
      source: EpochPositionSource::Genesis,
    };
    (
      EpochPositions {
        confirmed: genesis,
        finalized: genesis,
        checkpoint: genesis,
      },
      LOCAL_ERA_EPOCH_COUNT,
    )
  };
  let previous = previous.unwrap_or(baseline);

  let confirmed = baseline
    .confirmed
    .advance_to(latest_state.saturating_sub(depths.confirmed));
  let finalized = previous
    .finalized
    .advance_to(latest_state.saturating_sub(depths.finalized));
  // The remote checkpoint baseline is independent of remote PoS finality.
  let checkpoint = previous
    .checkpoint
    .advance_to(align_to_era(finalized.height, era_epoch_count));

  EpochPositions {
    confirmed,
    finalized,
    checkpoint,
  }
}

fn align_to_era(height: BlockHeight, era_epoch_count: NonZeroU64) -> BlockHeight {
  height - height % era_epoch_count.get()
}
