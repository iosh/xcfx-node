//! Parses account sources and derives keys before node startup acquires resources.

use std::{fmt, str::FromStr};

use cfxkey::{KeyPair, Secret};
use coins_bip32::{
  BIP32_HARDEN, ecdsa,
  path::DerivationPath,
  primitives::XKeyInfo,
  xkeys::{Parent, XPriv},
};
use coins_bip39::{English, Mnemonic, MnemonicError};
use thiserror::Error;

use super::{SigningKeyConflict, SigningKeys};

const DEVELOPMENT_MNEMONIC: &str = "test test test test test test test test test test test junk";
const DEFAULT_DERIVATION_PATH_PREFIX: &str = "m/44'/60'/0'/0";

/// Account configuration errors never retain private keys or mnemonic text.
#[derive(Debug, Error)]
pub(crate) enum AccountConfigError {
  #[error("expected a valid English BIP-39 mnemonic")]
  InvalidMnemonic,
  #[error("private key at index {index} must be a valid 32-byte secp256k1 private key")]
  InvalidPrivateKey { index: usize },
  #[error("expected an absolute BIP-32 path prefix with 31-bit indices")]
  InvalidDerivationPath,
  #[error("at most 254 derivation levels may precede the account index")]
  DerivationPathTooDeep,
  #[error("account count exceeds the non-hardened BIP-32 index range")]
  AccountCountOutOfRange,
  #[error("could not derive {context}: {source}")]
  KeyDerivation {
    context: String,
    #[source]
    source: MnemonicError,
  },
  #[error("non-hardened account indices were exhausted before generating all requested accounts")]
  AccountIndexExhausted,
  #[error("could not register signing key: {0}")]
  ConflictingKey(#[from] SigningKeyConflict),
}

/// A validated English mnemonic. Formatting never reveals its contents.
pub(crate) struct MnemonicPhrase(Mnemonic<English>);

impl Default for MnemonicPhrase {
  fn default() -> Self {
    DEVELOPMENT_MNEMONIC
      .parse()
      .expect("the public development mnemonic must be valid")
  }
}

impl FromStr for MnemonicPhrase {
  type Err = AccountConfigError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    let phrase = value.split_whitespace().collect::<Vec<_>>().join(" ");
    // The library's errors may contain the original phrase or an individual word.
    Mnemonic::new_from_phrase(&phrase)
      .map(Self)
      .map_err(|_| AccountConfigError::InvalidMnemonic)
  }
}

impl fmt::Debug for MnemonicPhrase {
  fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("MnemonicPhrase([REDACTED])")
  }
}

impl MnemonicPhrase {
  fn derive_parent_key(self, prefix: DerivationPathPrefix) -> Result<XPriv, AccountConfigError> {
    // Both inputs are already parsed, so this call cannot produce phrase parsing errors.
    self
      .0
      .derive_key(prefix.0, None)
      .map_err(|source| AccountConfigError::KeyDerivation {
        context: "the account parent key".to_owned(),
        source,
      })
  }
}

/// An absolute parent path for a sequence of non-hardened account indices.
#[derive(Clone, Debug)]
pub(crate) struct DerivationPathPrefix(DerivationPath);

impl Default for DerivationPathPrefix {
  fn default() -> Self {
    DEFAULT_DERIVATION_PATH_PREFIX
      .parse()
      .expect("the default account derivation path must be valid")
  }
}

impl FromStr for DerivationPathPrefix {
  type Err = AccountConfigError;

  fn from_str(value: &str) -> Result<Self, Self::Err> {
    let value = value.strip_suffix('/').unwrap_or(value);
    let mut components = value.split('/');
    if components.next() != Some("m") {
      return Err(AccountConfigError::InvalidDerivationPath);
    }

    let mut indices = Vec::new();
    for component in components {
      // BIP-32 stores depth in a byte. Reserve one level for the account index.
      if indices.len() == usize::from(u8::MAX) - 1 {
        return Err(AccountConfigError::DerivationPathTooDeep);
      }
      let hardened = component.ends_with('\'') || component.ends_with('h');
      let digits = if hardened {
        &component[..component.len() - 1]
      } else {
        component
      };
      if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AccountConfigError::InvalidDerivationPath);
      }
      let index = digits
        .parse::<u32>()
        .map_err(|_| AccountConfigError::InvalidDerivationPath)?;
      if index >= BIP32_HARDEN {
        return Err(AccountConfigError::InvalidDerivationPath);
      }
      indices.push(if hardened {
        index | BIP32_HARDEN
      } else {
        index
      });
    }

    // Construct from checked indices: the library's text parser can overflow
    // when adding the hardened offset to an unbounded input index.
    Ok(Self(DerivationPath::from(indices)))
  }
}

/// Parsed mnemonic settings. The account count is checked before deriving keys.
pub(crate) struct MnemonicAccountConfig {
  /// `None` selects the public development phrase and preserves its implicit origin.
  pub(crate) phrase: Option<MnemonicPhrase>,
  pub(crate) count: u32,
  pub(crate) derivation_path_prefix: DerivationPathPrefix,
}

impl Default for MnemonicAccountConfig {
  fn default() -> Self {
    Self {
      phrase: None,
      count: 10,
      derivation_path_prefix: DerivationPathPrefix::default(),
    }
  }
}

impl MnemonicAccountConfig {
  /// Zero accounts is valid. Every generated index must remain non-hardened.
  fn derive_signing_keys(self) -> Result<SigningKeys, AccountConfigError> {
    if self.count > BIP32_HARDEN {
      return Err(AccountConfigError::AccountCountOutOfRange);
    }
    let mut keys = SigningKeys::default();
    if self.count == 0 {
      return Ok(keys);
    }
    let parent = self
      .phrase
      .unwrap_or_default()
      .derive_parent_key(self.derivation_path_prefix)?;

    let mut next_index = 0;
    while keys.len() < self.count as usize {
      if next_index >= BIP32_HARDEN {
        return Err(AccountConfigError::AccountIndexExhausted);
      }
      let child =
        parent
          .derive_child(next_index)
          .map_err(|source| AccountConfigError::KeyDerivation {
            context: format!("an account starting at index {next_index}"),
            source: source.into(),
          })?;
      let info: &XKeyInfo = child.as_ref();
      if info.index >= BIP32_HARDEN {
        return Err(AccountConfigError::AccountIndexExhausted);
      }
      // BIP-32 can skip an invalid child. Continue after the actual returned index.
      next_index = info.index + 1;

      keys.add(to_conflux_key_pair(&child))?;
    }
    Ok(keys)
  }
}

/// Mutually exclusive account sources; balances are configured separately.
pub(crate) enum AccountSource {
  Mnemonic(MnemonicAccountConfig),
  PrivateKeys(Vec<KeyPair>),
}

impl Default for AccountSource {
  fn default() -> Self {
    Self::Mnemonic(MnemonicAccountConfig::default())
  }
}

impl AccountSource {
  /// Parses a private-key list without retaining its text in configuration or errors.
  /// Error indices are zero-based positions in the input list.
  pub(crate) fn parse_private_keys<S: AsRef<str>>(
    values: impl IntoIterator<Item = S>,
  ) -> Result<Self, AccountConfigError> {
    let keys = values
      .into_iter()
      .enumerate()
      .map(|(index, value)| {
        parse_private_key(value.as_ref())
          .map_err(|_| AccountConfigError::InvalidPrivateKey { index })
      })
      .collect::<Result<Vec<_>, _>>()?;
    Ok(Self::PrivateKeys(keys))
  }

  /// Records explicit user input even when it equals the public development phrase.
  pub(crate) fn has_explicit_key_material(&self) -> bool {
    match self {
      Self::Mnemonic(accounts) => accounts.phrase.is_some(),
      Self::PrivateKeys(keys) => !keys.is_empty(),
    }
  }

  /// Validates the count and builds signing keys before startup acquires resources.
  /// Consumes the source, retaining only the keys needed for Runtime signing.
  /// Repeated private keys keep their first configured position.
  pub(crate) fn build_signing_keys(self) -> Result<SigningKeys, AccountConfigError> {
    match self {
      Self::Mnemonic(accounts) => accounts.derive_signing_keys(),
      Self::PrivateKeys(configured) => {
        let mut keys = SigningKeys::default();
        for key in configured {
          keys.add(key)?;
        }
        Ok(keys)
      }
    }
  }
}

fn to_conflux_key_pair(key: &XPriv) -> KeyPair {
  let signing_key: &ecdsa::SigningKey = key.as_ref();
  KeyPair::from_secret_slice(&signing_key.to_bytes())
    .expect("a derived secp256k1 private key must be valid for Conflux")
}

/// Parses one hex private key, with an optional `0x` prefix, without retaining text.
fn parse_private_key(value: &str) -> Result<KeyPair, cfxkey::Error> {
  let encoded = value.strip_prefix("0x").unwrap_or(value);
  if encoded.len() != 64 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
    return Err(cfxkey::Error::InvalidSecret);
  }
  let secret: Secret = encoded.parse()?;
  KeyPair::from_secret(secret)
}
