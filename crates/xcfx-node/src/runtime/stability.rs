//! Explicit local stability markers, independent of protocol execution inputs.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StabilitySource {
  Genesis,
  Fork,
  Developer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StablePosition {
  pub(crate) height: BlockHeight,
  pub(crate) source: StabilitySource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Stability {
  pub(crate) confirmed: StablePosition,
  pub(crate) finalized: StablePosition,
  pub(crate) checkpoint: StablePosition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StabilityKind {
  Confirmed,
  Finalized,
  Checkpoint,
}

#[derive(Debug, Error)]
#[error("stability height {height} must be between {minimum} and available state {maximum}")]
pub(crate) struct InvalidStabilityHeight {
  pub(crate) height: BlockHeight,
  pub(crate) minimum: BlockHeight,
  pub(crate) maximum: BlockHeight,
}

impl Stability {
  pub(crate) fn initial(view: &EpochView) -> Self {
    if let Some(base) = view.fork_base() {
      return base.stability;
    }
    let genesis = StablePosition {
      height: view.epoch_height(),
      source: StabilitySource::Genesis,
    };
    Self {
      confirmed: genesis,
      finalized: genesis,
      checkpoint: genesis,
    }
  }

  pub(crate) fn reorg_boundary(&self) -> BlockHeight {
    self.finalized.height.max(self.checkpoint.height)
  }

  pub(crate) fn after_reorg(mut self, ancestor: BlockHeight) -> Self {
    if self.confirmed.height > ancestor {
      self.confirmed = StablePosition {
        height: ancestor,
        source: StabilitySource::Developer,
      };
    }
    self
  }
}

impl NodeRuntime {
  pub(crate) fn stability(&self) -> Stability {
    self.runtime_state.stability
  }

  /// Advances a test marker without changing PoS state or claiming network consensus.
  pub(crate) fn advance_stability(
    &mut self,
    kind: StabilityKind,
    height: BlockHeight,
  ) -> Result<Stability, InvalidStabilityHeight> {
    let maximum = self.runtime_state.history.latest_state_height();
    let position = match kind {
      StabilityKind::Confirmed => &mut self.runtime_state.stability.confirmed,
      StabilityKind::Finalized => &mut self.runtime_state.stability.finalized,
      StabilityKind::Checkpoint => &mut self.runtime_state.stability.checkpoint,
    };
    if height < position.height || height > maximum {
      return Err(InvalidStabilityHeight {
        height,
        minimum: position.height,
        maximum,
      });
    }
    if height > position.height {
      *position = StablePosition {
        height,
        source: StabilitySource::Developer,
      };
    }
    Ok(self.runtime_state.stability)
  }

  /// Releases detached topology. Checkpoints and outstanding views keep their own pins.
  pub(crate) fn prune_detached_blocks(&mut self) -> usize {
    let graph = &mut self.runtime_state.graph;
    let before = graph.len();
    graph.retain_ancestors([self
      .runtime_state
      .history
      .optimistic_head()
      .state()
      .epoch_id]);
    before - graph.len()
  }

  pub(crate) fn retained_block_count(&self) -> usize {
    self.runtime_state.graph.len()
  }
}
