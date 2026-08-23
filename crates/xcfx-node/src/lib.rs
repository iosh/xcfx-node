mod execution;
mod genesis;
mod mpt;
mod pos;
mod runtime;
mod state;
mod transaction_ingress;
mod transaction_pool;
/// Identifies the Rust product code included in the current build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuildInfo {
  /// Version of the `xcfx-node` Rust crate.
  pub version: &'static str,
  /// Rust target triple used to compile the crate.
  pub target: &'static str,
}

/// Returns deterministic metadata for the current Rust build.
#[must_use]
pub const fn build_info() -> BuildInfo {
  BuildInfo {
    version: env!("CARGO_PKG_VERSION"),
    target: env!("XCFX_NODE_BUILD_TARGET"),
  }
}

#[cfg(test)]
mod tests {
  use super::build_info;

  #[test]
  fn reports_manifest_version() {
    assert!(!build_info().version.is_empty());
  }

  #[test]
  fn reports_compilation_identity() {
    let info = build_info();
    assert!(info.target.starts_with(std::env::consts::ARCH));
  }
}
