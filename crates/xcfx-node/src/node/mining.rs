//! Mining policy and a timer that submits at most one tick per armed deadline.

use std::{sync::Arc, time::Duration};

use tokio::{
  sync::{mpsc, watch},
  time::{Instant, sleep_until},
};

use crate::{config::MiningMode, runtime::RuntimeCommitOutcome};

use super::{
  command::{CallId, InvalidMiningInterval, MessageId, MiningError, Work},
  lifecycle::Lifecycle,
};

#[derive(Clone, Copy, Debug)]
pub(crate) enum MiningCause {
  Manual(CallId),
  Automatic(MessageId),
  Interval,
}

/// Facts are emitted only after Runtime commit. Connection-level response
/// ordering and subscription projection belong to the consumer.
pub(crate) enum NodeEvent {
  Committed {
    cause: MiningCause,
    outcome: Arc<RuntimeCommitOutcome>,
  },
  MiningFailed {
    cause: MiningCause,
    error: Arc<MiningError>,
  },
}

struct Interval {
  period: Duration,
}

/// Arc identity distinguishes replacements, even when their periods are equal.
pub(super) struct Tick {
  interval: Arc<Interval>,
}

#[derive(Clone)]
pub(super) struct TimerSchedule {
  interval: Arc<Interval>,
  deadline: Instant,
}

enum MiningState {
  Auto,
  Manual,
  Interval(TimerSchedule),
}

pub(super) fn interval_deadline(period: Duration) -> Result<Instant, InvalidMiningInterval> {
  Instant::now()
    .checked_add(period)
    .ok_or(InvalidMiningInterval)
}

impl MiningState {
  fn new(mode: MiningMode) -> Result<Self, InvalidMiningInterval> {
    match mode {
      MiningMode::Auto => Ok(Self::Auto),
      MiningMode::Manual => Ok(Self::Manual),
      MiningMode::Interval(period) => {
        assert!(
          !period.is_zero(),
          "interval mining requires a nonzero period"
        );
        let deadline = interval_deadline(period)?;
        Ok(Self::Interval(TimerSchedule {
          interval: Arc::new(Interval { period }),
          deadline,
        }))
      }
    }
  }
}

pub(super) struct MiningScheduler {
  state: MiningState,
  schedule: watch::Sender<Option<TimerSchedule>>,
}

impl MiningScheduler {
  /// Construction checks the initial deadline without starting the timer.
  pub(super) fn new(
    mode: MiningMode,
    schedule: watch::Sender<Option<TimerSchedule>>,
  ) -> Result<Self, InvalidMiningInterval> {
    Ok(Self {
      state: MiningState::new(mode)?,
      schedule,
    })
  }

  pub(super) fn mode(&self) -> MiningMode {
    match &self.state {
      MiningState::Auto => MiningMode::Auto,
      MiningState::Manual => MiningMode::Manual,
      MiningState::Interval(schedule) => MiningMode::Interval(schedule.interval.period),
    }
  }

  pub(super) fn publish_schedule(&self) {
    let schedule = match &self.state {
      MiningState::Interval(schedule) => Some(schedule.clone()),
      MiningState::Auto | MiningState::Manual => None,
    };
    self.schedule.send_replace(schedule);
  }

  pub(super) fn set_automine(&mut self, enabled: bool) {
    if enabled {
      self.state = MiningState::Auto;
    } else if matches!(self.state, MiningState::Auto) {
      self.state = MiningState::Manual;
    } else {
      return;
    }
    self.publish_schedule();
  }

  pub(super) fn set_interval(&mut self, period: Duration) -> Result<(), InvalidMiningInterval> {
    let mode = if period.is_zero() {
      MiningMode::Manual
    } else {
      MiningMode::Interval(period)
    };
    let state = MiningState::new(mode)?;
    self.state = state;
    self.publish_schedule();
    Ok(())
  }

  pub(super) fn accepts(&self, tick: &Tick) -> bool {
    matches!(
      &self.state,
      MiningState::Interval(schedule) if Arc::ptr_eq(&schedule.interval, &tick.interval)
    )
  }

  pub(super) fn rearm(&mut self, tick: &Tick) -> Result<(), InvalidMiningInterval> {
    if !self.accepts(tick) {
      return Ok(());
    }
    let Ok(deadline) = interval_deadline(tick.interval.period) else {
      // Do not keep a due timer live when the next deadline is unrepresentable.
      self.state = MiningState::Manual;
      self.publish_schedule();
      return Err(InvalidMiningInterval);
    };
    self.state = MiningState::Interval(TimerSchedule {
      interval: Arc::clone(&tick.interval),
      deadline,
    });
    self.publish_schedule();
    Ok(())
  }
}

pub(super) async fn run_timer(
  requests: mpsc::Sender<Work>,
  mut schedules: watch::Receiver<Option<TimerSchedule>>,
  lifecycle: Arc<Lifecycle>,
) {
  loop {
    let schedule = schedules.borrow_and_update().clone();
    let Some(schedule) = schedule else {
      tokio::select! {
        biased;
        _ = lifecycle.stopped() => return,
        changed = schedules.changed() => {
          if changed.is_err() { return; }
        }
      }
      continue;
    };

    tokio::select! {
      biased;
      _ = lifecycle.stopped() => return,
      changed = schedules.changed() => {
        if changed.is_err() { return; }
        continue;
      }
      _ = sleep_until(schedule.deadline) => {}
    }

    let permit = tokio::select! {
      biased;
      _ = lifecycle.stopped() => return,
      changed = schedules.changed() => {
        if changed.is_err() { return; }
        continue;
      }
      permit = requests.reserve() => {
        let Ok(permit) = permit else { return; };
        permit
      }
    };
    let tick = Tick {
      interval: schedule.interval,
    };
    if lifecycle.admit(permit, Work::Tick(tick), None).is_err() {
      return;
    }

    // Only execution completion or mode replacement arms another deadline.
    tokio::select! {
      biased;
      _ = lifecycle.stopped() => return,
      changed = schedules.changed() => {
        if changed.is_err() { return; }
      }
    }
  }
}
