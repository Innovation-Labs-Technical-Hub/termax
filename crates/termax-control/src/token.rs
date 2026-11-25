//! Per-client bearer tokens.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::error::ControlError;

/// Random bytes per token (256 bits).
pub const TOKEN_BYTES: usize = 32;

/// A client token: 64 lowercase hex characters from the OS CSPRNG. Zeroed
/// on drop; `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(Zeroizing<String>);

impl Token {
    /// A fresh random token.
    pub fn generate() -> Result<Self, ControlError> {
        let mut bytes = Zeroizing::new([0u8; TOKEN_BYTES]);
        getrandom::fill(bytes.as_mut()).map_err(|e| ControlError::Random(e.to_string()))?;
        Ok(Self(Zeroizing::new(hex::encode(bytes.as_ref()))))
    }

    /// Wrap a stored token.
    pub fn from_secret(s: String) -> Self {
        Self(Zeroizing::new(s))
    }

    /// The secret text, for the keychain and the wire only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Hex SHA-256 of the token: what the server stores.
    pub fn hash(&self) -> String {
        hex::encode(Sha256::digest(self.0.as_bytes()))
    }

    /// Whether this token hashes to `stored` (constant time).
    pub fn matches(&self, stored: &str) -> bool {
        self.hash().as_bytes().ct_eq(stored.as_bytes()).into()
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([REDACTED])")
    }
}

impl Serialize for Token {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Token {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self(Zeroizing::new(String::deserialize(d)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_256_bit_random_hex_and_never_printed() {
        let a = Token::generate().unwrap();
        let b = Token::generate().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.expose().len(), TOKEN_BYTES * 2);
        assert!(a.expose().bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(format!("{a:?}"), "Token([REDACTED])");
        assert!(!format!("{a:?}").contains(a.expose()));
    }

    #[test]
    fn only_the_right_token_matches_its_hash() {
        let a = Token::generate().unwrap();
        let stored = a.hash();
        assert_ne!(stored, a.expose());
        assert!(a.matches(&stored));
        assert!(!Token::generate().unwrap().matches(&stored));
        assert!(!a.matches(""));
    }
}
