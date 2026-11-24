//! Key material, key derivation and record encryption.
//!
//! * Account key = HKDF-SHA256(salt = Secret Key, ikm = Argon2id(password, kdf_salt)).
//!   Both secrets are required; neither alone narrows the search space.
//! * Record ciphertext = `version (1) || nonce (24) || XChaCha20-Poly1305(ct || tag)`.
//!   The associated data always starts with a domain tag and the version byte,
//!   followed by length-prefixed context fields (table, column, record id, and
//!   any plaintext index columns of that row).

use std::fmt;
use std::str::FromStr;

use argon2::{Algorithm, Argon2, Version};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use data_encoding::BASE32_NOPAD;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretBox};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

pub(crate) const KEY_LEN: usize = 32;
pub(crate) const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const CT_VERSION: u8 = 1;
const AAD_DOMAIN: &[u8] = b"oxterm-vault";
const ACCOUNT_KEY_INFO: &[u8] = b"oxterm-vault account-key v1";

/// 256-bit symmetric key, wiped from memory on drop.
pub(crate) type Key = Zeroizing<[u8; KEY_LEN]>;

/// Identifier stored in the header for the only KDF supported today.
pub(crate) const KDF_ARGON2ID: &str = "argon2id-v19";

/// Argon2id cost parameters. Stored in the store header next to the salt so
/// they can be raised later (see [`crate::VaultStore::change_password_with_params`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    m_cost_kib: u32,
    t_cost: u32,
    p_cost: u32,
}

impl KdfParams {
    /// Validated constructor. Use [`KdfParams::default`] in production; cheap
    /// values are only appropriate for tests.
    pub fn new(m_cost_kib: u32, t_cost: u32, p_cost: u32) -> Result<Self> {
        let p = Self {
            m_cost_kib,
            t_cost,
            p_cost,
        };
        p.to_argon2()?;
        Ok(p)
    }

    /// Memory cost in KiB.
    pub fn m_cost_kib(&self) -> u32 {
        self.m_cost_kib
    }

    /// Number of passes.
    pub fn t_cost(&self) -> u32 {
        self.t_cost
    }

    /// Degree of parallelism (lanes).
    pub fn p_cost(&self) -> u32 {
        self.p_cost
    }

    fn to_argon2(self) -> Result<argon2::Params> {
        argon2::Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(KEY_LEN))
            .map_err(|e| Error::InvalidKdfParams(e.to_string()))
    }
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes, 4 lanes (RFC 9106 second recommended option).
    fn default() -> Self {
        Self {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            p_cost: 4,
        }
    }
}

/// 128-bit random Secret Key, the second factor of the account key.
///
/// It is generated once per account, never stored in the database, and shown
/// to the user in the form `OX1-XXXXXX-XXXXX-XXXXX-XXXXX-XXXXX` (RFC 4648
/// base32). `Debug` output is redacted.
pub struct SecretKey(SecretBox<[u8; 16]>);

const SECRET_KEY_PREFIX: &str = "OX1";
const SECRET_KEY_CHARS: usize = 26;
const SECRET_KEY_GROUPS: [usize; 5] = [6, 5, 5, 5, 5];

impl SecretKey {
    /// Generate a fresh Secret Key from the OS RNG.
    pub fn generate() -> Result<Self> {
        let mut bytes = Box::new([0u8; 16]);
        fill_random(bytes.as_mut())?;
        Ok(Self(SecretBox::new(bytes)))
    }

    /// Wrap raw key bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(SecretBox::new(Box::new(bytes)))
    }

    /// Parse the human-readable form. Case, dashes and whitespace are
    /// ignored, and the look-alikes `0`, `1`, `8` are read as `O`, `I`, `B`.
    pub fn parse(s: &str) -> Result<Self> {
        let compact: Zeroizing<String> = Zeroizing::new(
            s.chars()
                .filter(|c| !c.is_whitespace() && *c != '-')
                .map(|c| c.to_ascii_uppercase())
                .collect(),
        );
        let body = compact
            .strip_prefix(SECRET_KEY_PREFIX)
            .ok_or(Error::InvalidSecretKey)?;
        if body.len() != SECRET_KEY_CHARS {
            return Err(Error::InvalidSecretKey);
        }
        let normalized: Zeroizing<String> = Zeroizing::new(
            body.chars()
                .map(|c| match c {
                    '0' => 'O',
                    '1' => 'I',
                    '8' => 'B',
                    other => other,
                })
                .collect(),
        );
        let decoded = Zeroizing::new(
            BASE32_NOPAD
                .decode(normalized.as_bytes())
                .map_err(|_| Error::InvalidSecretKey)?,
        );
        let bytes: [u8; 16] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| Error::InvalidSecretKey)?;
        Ok(Self::from_bytes(bytes))
    }

    /// Human-readable form for display or an emergency kit.
    pub fn to_formatted(&self) -> Zeroizing<String> {
        let encoded = Zeroizing::new(BASE32_NOPAD.encode(self.expose()));
        let mut out = Zeroizing::new(String::with_capacity(40));
        out.push_str(SECRET_KEY_PREFIX);
        let mut rest: &str = &encoded;
        for n in SECRET_KEY_GROUPS {
            let (head, tail) = rest.split_at(n);
            out.push('-');
            out.push_str(head);
            rest = tail;
        }
        out
    }

    pub(crate) fn expose(&self) -> &[u8; 16] {
        self.0.expose_secret()
    }
}

impl Clone for SecretKey {
    fn clone(&self) -> Self {
        Self::from_bytes(*self.expose())
    }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretKey([REDACTED])")
    }
}

impl FromStr for SecretKey {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

/// Generate a new random Secret Key (see [`SecretKey`]).
pub fn generate_secret_key() -> Result<SecretKey> {
    SecretKey::generate()
}

pub(crate) fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|e| Error::Rng(e.to_string()))
}

pub(crate) fn random_key() -> Result<Key> {
    let mut k = Zeroizing::new([0u8; KEY_LEN]);
    fill_random(k.as_mut())?;
    Ok(k)
}

/// Derive the account key from both user secrets.
pub(crate) fn derive_account_key(
    password: &str,
    secret_key: &SecretKey,
    salt: &[u8],
    params: KdfParams,
) -> Result<Key> {
    let mut stretched: Key = Zeroizing::new([0u8; KEY_LEN]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params.to_argon2()?)
        .hash_password_into(password.as_bytes(), salt, stretched.as_mut())
        .map_err(|e| Error::Crypto(format!("argon2: {e}")))?;
    let hk = Hkdf::<Sha256>::new(Some(secret_key.expose()), stretched.as_ref());
    let mut out: Key = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(ACCOUNT_KEY_INFO, out.as_mut())
        .map_err(|e| Error::Crypto(format!("hkdf: {e}")))?;
    Ok(out)
}

/// Derive an independent sub-key for a single purpose.
pub(crate) fn derive_subkey(key: &Key, info: &[u8]) -> Result<Key> {
    let hk = Hkdf::<Sha256>::new(None, key.as_ref());
    let mut out: Key = Zeroizing::new([0u8; KEY_LEN]);
    hk.expand(info, out.as_mut())
        .map_err(|e| Error::Crypto(format!("hkdf: {e}")))?;
    Ok(out)
}

/// Keyed, deterministic token (hex HMAC-SHA256) for equality lookups on
/// values that must not appear in plaintext.
pub(crate) fn mac_token(key: &Key, data: &[u8]) -> Result<String> {
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(key.as_ref())
        .map_err(|e| Error::Crypto(format!("hmac: {e}")))?;
    mac.update(data);
    Ok(data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes()))
}

/// Associated data builder. Fields are length-prefixed, so no two distinct
/// field lists can encode to the same bytes.
pub(crate) struct Aad(Vec<u8>);

impl Aad {
    pub(crate) fn new(table: &str, column: &str) -> Self {
        let mut v = Vec::with_capacity(128);
        v.extend_from_slice(AAD_DOMAIN);
        v.push(CT_VERSION);
        Self(v).field(table).field(column)
    }

    pub(crate) fn field(mut self, value: impl AsRef<[u8]>) -> Self {
        let value = value.as_ref();
        self.0
            .extend_from_slice(&(value.len() as u64).to_be_bytes());
        self.0.extend_from_slice(value);
        self
    }
}

/// Failed AEAD authentication; callers decide whether that means bad
/// credentials or corruption.
#[derive(Debug)]
pub(crate) struct DecryptError;

fn cipher(key: &Key) -> Result<XChaCha20Poly1305> {
    XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|e| Error::Crypto(e.to_string()))
}

/// Encrypt `plaintext` with a fresh random nonce.
pub(crate) fn seal(key: &Key, plaintext: &[u8], aad: &Aad) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    fill_random(&mut nonce)?;
    let ct = cipher(key)?
        .encrypt(
            &nonce.into(),
            Payload {
                msg: plaintext,
                aad: &aad.0,
            },
        )
        .map_err(|e| Error::Crypto(e.to_string()))?;
    let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
    out.push(CT_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Decrypt a blob produced by [`seal`] with the same key and AAD.
pub(crate) fn open(
    key: &Key,
    blob: &[u8],
    aad: &Aad,
) -> std::result::Result<Zeroizing<Vec<u8>>, DecryptError> {
    if blob.len() < 1 + NONCE_LEN || blob[0] != CT_VERSION {
        return Err(DecryptError);
    }
    let nonce: [u8; NONCE_LEN] = blob[1..1 + NONCE_LEN]
        .try_into()
        .map_err(|_| DecryptError)?;
    cipher(key)
        .map_err(|_| DecryptError)?
        .decrypt(
            &nonce.into(),
            Payload {
                msg: &blob[1 + NONCE_LEN..],
                aad: &aad.0,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| DecryptError)
}

/// Wrap (encrypt) a key under another key.
pub(crate) fn wrap_key(wrapping: &Key, key: &Key, aad: &Aad) -> Result<Vec<u8>> {
    seal(wrapping, key.as_ref(), aad)
}

/// Unwrap a key produced by [`wrap_key`].
pub(crate) fn unwrap_key(
    wrapping: &Key,
    blob: &[u8],
    aad: &Aad,
) -> std::result::Result<Key, DecryptError> {
    let plain = open(wrapping, blob, aad)?;
    let mut key: Key = Zeroizing::new([0u8; KEY_LEN]);
    if plain.len() != KEY_LEN {
        return Err(DecryptError);
    }
    key.copy_from_slice(&plain);
    Ok(key)
}

/// Serialize to JSON and encrypt; the intermediate plaintext is wiped.
pub(crate) fn seal_json<T: Serialize>(key: &Key, value: &T, aad: &Aad) -> Result<Vec<u8>> {
    let plain = Zeroizing::new(
        serde_json::to_vec(value).map_err(|e| Error::Crypto(format!("encode: {e}")))?,
    );
    seal(key, &plain, aad)
}

/// Decrypt and deserialize; authentication or decoding failures are reported
/// as [`Error::Corrupt`] naming `what`.
pub(crate) fn open_json<T: DeserializeOwned>(
    key: &Key,
    blob: &[u8],
    aad: &Aad,
    what: &str,
) -> Result<T> {
    let plain = open(key, blob, aad)
        .map_err(|_| Error::Corrupt(format!("{what}: authentication failed")))?;
    serde_json::from_slice(&plain).map_err(|e| Error::Corrupt(format!("{what}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> KdfParams {
        KdfParams::new(64, 1, 1).unwrap()
    }

    #[test]
    fn secret_key_round_trips_through_formatted_string() {
        let sk = generate_secret_key().unwrap();
        let s = sk.to_formatted();
        assert!(s.starts_with("OX1-"));
        assert_eq!(s.len(), 3 + 5 + 26);
        let back = SecretKey::parse(&s.to_lowercase()).unwrap();
        assert_eq!(back.expose(), sk.expose());
    }

    #[test]
    fn secret_key_rejects_garbage_and_redacts_debug() {
        assert!(matches!(
            SecretKey::parse("OX1-AAAA"),
            Err(Error::InvalidSecretKey)
        ));
        assert!(matches!(
            SecretKey::parse("XX9-AAAAAA-AAAAA-AAAAA-AAAAA-AAAAA"),
            Err(Error::InvalidSecretKey)
        ));
        let sk = SecretKey::from_bytes([7; 16]);
        assert_eq!(format!("{sk:?}"), "SecretKey([REDACTED])");
    }

    #[test]
    fn secret_key_accepts_lookalike_digits() {
        // 'I', 'O' and 'B' typed as the digits '1', '0' and '8'.
        let sk = SecretKey::parse("OX1-IOBAAA-AAAAA-AAAAA-AAAAA-AAAAA").unwrap();
        let typed = SecretKey::parse("ox1 108aaa aaaaa aaaaa aaaaa aaaaa").unwrap();
        assert_eq!(sk.expose(), typed.expose());
    }

    #[test]
    fn account_key_depends_on_both_secrets() {
        let salt = [1u8; SALT_LEN];
        let sk1 = SecretKey::from_bytes([1; 16]);
        let sk2 = SecretKey::from_bytes([2; 16]);
        let a = derive_account_key("pw", &sk1, &salt, cheap()).unwrap();
        let b = derive_account_key("pw", &sk2, &salt, cheap()).unwrap();
        let c = derive_account_key("pw2", &sk1, &salt, cheap()).unwrap();
        let a2 = derive_account_key("pw", &sk1, &salt, cheap()).unwrap();
        assert_eq!(*a, *a2);
        assert_ne!(*a, *b);
        assert_ne!(*a, *c);
    }

    #[test]
    fn seal_uses_fresh_nonces_and_binds_aad() {
        let key = random_key().unwrap();
        let aad = Aad::new("t", "c").field("id1");
        let x = seal(&key, b"hello", &aad).unwrap();
        let y = seal(&key, b"hello", &aad).unwrap();
        assert_ne!(x, y);
        assert_eq!(&*open(&key, &x, &aad).unwrap(), b"hello");
        assert!(open(&key, &x, &Aad::new("t", "c").field("id2")).is_err());
        let other = random_key().unwrap();
        assert!(open(&other, &x, &aad).is_err());
    }

    #[test]
    fn aad_fields_are_unambiguous() {
        let a = Aad::new("t", "c").field("ab").field("c");
        let b = Aad::new("t", "c").field("a").field("bc");
        assert_ne!(a.0, b.0);
    }

    #[test]
    fn kdf_params_are_validated() {
        assert!(matches!(
            KdfParams::new(1, 1, 1),
            Err(Error::InvalidKdfParams(_))
        ));
        assert!(KdfParams::new(64, 1, 1).is_ok());
    }
}
