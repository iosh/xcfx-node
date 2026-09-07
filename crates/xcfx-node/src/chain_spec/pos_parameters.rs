use diem_types::{
  block_info::Round,
  term_state::{
    IN_QUEUE_LOCKED_VIEWS, OUT_QUEUE_LOCKED_VIEWS, ROUND_PER_TERM, TERM_ELECTED_SIZE,
    TERM_MAX_SIZE, pos_state_config::PosStateConfig,
  },
};

/// PoS term, lockup, and protocol upgrade parameters.
#[derive(Clone, Debug)]
pub(crate) struct PosParameters {
  pub(crate) round_per_term: Round,
  pub(crate) term_max_size: usize,
  pub(crate) term_elected_size: usize,
  pub(crate) in_queue_locked_views: u64,
  pub(crate) out_queue_locked_views: u64,
  pub(crate) cip99_transition_view: u64,
  pub(crate) cip99_in_queue_locked_views: u64,
  pub(crate) cip99_out_queue_locked_views: u64,
  pub(crate) fix_cip99_transition_view: u64,
  pub(crate) fix_cip99_in_queue_locked_views: u64,
  pub(crate) fix_cip99_out_queue_locked_views: u64,
  pub(crate) nonce_limit_transition_view: u64,
  pub(crate) max_nonce_per_account: u64,
  pub(crate) cip136_transition_view: u64,
  pub(crate) cip136_in_queue_locked_views: u64,
  pub(crate) cip136_out_queue_locked_views: u64,
  pub(crate) cip136_round_per_term: u64,
  pub(crate) cip156_transition_view: u64,
  pub(crate) cip156_dispute_locked_views: u64,
  pub(crate) cip173_transition_view: u64,
}

impl PosParameters {
  pub(crate) fn into_config(self) -> PosStateConfig {
    PosStateConfig::new(
      self.round_per_term,
      self.term_max_size,
      self.term_elected_size,
      self.in_queue_locked_views,
      self.out_queue_locked_views,
      self.cip99_transition_view,
      self.cip99_in_queue_locked_views,
      self.cip99_out_queue_locked_views,
      self.fix_cip99_transition_view,
      self.fix_cip99_in_queue_locked_views,
      self.fix_cip99_out_queue_locked_views,
      self.nonce_limit_transition_view,
      self.max_nonce_per_account,
      self.cip136_transition_view,
      self.cip136_in_queue_locked_views,
      self.cip136_out_queue_locked_views,
      self.cip136_round_per_term,
      self.cip156_transition_view,
      self.cip156_dispute_locked_views,
      self.cip173_transition_view,
    )
  }
}

/// Upstream base parameters with PoS upgrades left unscheduled.
///
/// Transition views use `u64::MAX`; a network preset must supply its own schedule.
impl Default for PosParameters {
  fn default() -> Self {
    Self {
      round_per_term: ROUND_PER_TERM,
      term_max_size: TERM_MAX_SIZE,
      term_elected_size: TERM_ELECTED_SIZE,
      in_queue_locked_views: IN_QUEUE_LOCKED_VIEWS,
      out_queue_locked_views: OUT_QUEUE_LOCKED_VIEWS,
      cip99_transition_view: u64::MAX,
      cip99_in_queue_locked_views: OUT_QUEUE_LOCKED_VIEWS,
      cip99_out_queue_locked_views: IN_QUEUE_LOCKED_VIEWS,
      fix_cip99_transition_view: u64::MAX,
      fix_cip99_in_queue_locked_views: OUT_QUEUE_LOCKED_VIEWS,
      fix_cip99_out_queue_locked_views: IN_QUEUE_LOCKED_VIEWS,
      nonce_limit_transition_view: u64::MAX,
      max_nonce_per_account: u64::MAX,
      cip136_transition_view: u64::MAX,
      cip136_in_queue_locked_views: OUT_QUEUE_LOCKED_VIEWS,
      cip136_out_queue_locked_views: IN_QUEUE_LOCKED_VIEWS,
      cip136_round_per_term: ROUND_PER_TERM,
      cip156_transition_view: u64::MAX,
      cip156_dispute_locked_views: u64::MAX,
      cip173_transition_view: u64::MAX,
    }
  }
}
