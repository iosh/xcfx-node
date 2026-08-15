//! Proofs for logical values in layered Conflux state.

use cfx_internal_common::StateRootWithAuxInfo;
use cfx_mpt::TrieProof;
use primitives::{MERKLE_NULL_NODE, StorageKeyWithSpace};

/// A proof of one logical key's value across the three Conflux state MPTs.
#[derive(Debug, Default)]
pub(crate) struct StateProof {
  pub(super) current_delta_proof: Option<TrieProof>,
  pub(super) intermediate_delta_proof: Option<TrieProof>,
  pub(super) snapshot_proof: Option<TrieProof>,
}

impl StateProof {
  /// Verifies that `value` is the logical value of `key` under the supplied
  /// state identity.
  pub(crate) fn is_valid_kv(
    &self,
    key: StorageKeyWithSpace<'_>,
    value: Option<&[u8]>,
    root_with_aux_info: &StateRootWithAuxInfo,
  ) -> bool {
    let root = &root_with_aux_info.state_root;
    let current_padding =
      StorageKeyWithSpace::delta_mpt_padding(&root.snapshot_root, &root.intermediate_delta_root);

    let current_key = key.to_delta_mpt_key_bytes(&current_padding);

    // A Delta MPT tombstone encodes a logical deletion.
    // Physical absence is verified separately before continuing below.
    let expected_delta_value = Some(value.unwrap_or_default());

    match &self.current_delta_proof {
      Some(proof) if proof.is_valid_kv(&current_key, expected_delta_value, &root.delta_root) => {
        return true;
      }
      Some(proof) if proof.is_valid_kv(&current_key, None, &root.delta_root) => {}
      Some(_) => return false,
      None if root.delta_root == MERKLE_NULL_NODE => {}
      None => return false,
    }

    match (
      &self.intermediate_delta_proof,
      root_with_aux_info
        .aux_info
        .maybe_intermediate_mpt_key_padding
        .as_ref(),
    ) {
      (Some(proof), Some(padding)) => {
        let intermediate_key = key.to_delta_mpt_key_bytes(padding);

        if proof.is_valid_kv(
          &intermediate_key,
          expected_delta_value,
          &root.intermediate_delta_root,
        ) {
          return true;
        }

        if !proof.is_valid_kv(&intermediate_key, None, &root.intermediate_delta_root) {
          return false;
        }
      }
      (Some(_), None) => return false,
      (None, _) if root.intermediate_delta_root == MERKLE_NULL_NODE => {}
      (None, _) => return false,
    }

    match &self.snapshot_proof {
      Some(proof) => proof.is_valid_kv(&key.to_key_bytes(), value, &root.snapshot_root),
      None => false,
    }
  }
}
