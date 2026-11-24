//! Host-key trust: verifiers (in-memory and vault-backed) and the user
//! decision callback for unknown keys.
//!
//! Policy, applied by the connect flow:
//! - [`HostKeyStatus::Trusted`] → continue.
//! - [`HostKeyStatus::Unknown`] → ask [`TrustDecision`]; save only on
//!   [`TrustChoice::AcceptAndSave`].
//! - [`HostKeyStatus::Changed`] and [`HostKeyStatus::Revoked`] → abort, never
//!   prompt.
//! - Any verifier error → abort (fail closed).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use russh::keys::{HashAlg, PublicKey};
use termax_vault::{KnownHost, VaultStore};

use crate::error::{Error, Result};

/// Boxed `Send` future used by the object-safe callback traits.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The server key being checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyInfo {
    /// Lookup key, `host:port` (`[v6addr]:port` for IPv6 literals), host
    /// lower-cased. Matches the `termax-vault` known-hosts format.
    pub host_port: String,
    /// Key type, e.g. `ssh-ed25519`, `ecdsa-sha2-nistp256`, `ssh-rsa`.
    pub key_type: String,
    /// OpenSSH SHA-256 fingerprint, `SHA256:<base64-no-pad>`.
    pub fingerprint: String,
}

impl HostKeyInfo {
    pub(crate) fn new(host: &str, port: u16, key: &PublicKey) -> Self {
        Self {
            host_port: host_port(host, port),
            key_type: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
        }
    }
}

/// `host:port` as used by the vault known-hosts table.
pub fn host_port(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Result of checking a host key against a trust store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// The exact key is trusted for this host.
    Trusted,
    /// No key is known for this host.
    Unknown {
        /// Fingerprint of the presented key.
        fingerprint: String,
    },
    /// A different key is trusted for this host.
    Changed {
        /// Type of the trusted key.
        old_key_type: String,
        /// Fingerprint of the trusted key.
        old: String,
        /// Fingerprint of the presented key.
        new: String,
    },
    /// The key is explicitly revoked.
    Revoked,
}

/// A trust store for host keys.
pub trait HostKeyVerifier: Send + Sync {
    /// Classify `key`.
    fn verify<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<HostKeyStatus>>;
    /// Persist `key` as trusted (called after [`TrustChoice::AcceptAndSave`]).
    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<()>>;
}

/// The user's answer for an unknown host key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChoice {
    /// Trust and remember the key.
    AcceptAndSave,
    /// Trust for this connection only.
    AcceptOnce,
    /// Abort the connection.
    Reject,
}

/// UI callback asked about unknown host keys. Changed and revoked keys never
/// reach it.
pub trait TrustDecision: Send + Sync {
    /// Decide whether to trust an unknown key.
    fn unknown_host<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, TrustChoice>;
}

/// A [`TrustDecision`] that always gives the same answer (useful for
/// non-interactive callers; `Reject` is the safe choice).
#[derive(Debug, Clone, Copy)]
pub struct FixedTrust(pub TrustChoice);

impl TrustDecision for FixedTrust {
    fn unknown_host<'a>(&'a self, _key: &'a HostKeyInfo) -> BoxFuture<'a, TrustChoice> {
        let c = self.0;
        Box::pin(async move { c })
    }
}

/// Shared classification. `known` holds `(key_type, fingerprint)` pairs
/// trusted for the host. An exact key-type match decides; otherwise any
/// other trusted key for the host means the identity changed (fail closed:
/// a server that suddenly presents a different key type is not "unknown").
pub(crate) fn classify(
    key: &HostKeyInfo,
    known: &[(String, String)],
    revoked: bool,
) -> HostKeyStatus {
    if revoked {
        return HostKeyStatus::Revoked;
    }
    if let Some((_, fp)) = known.iter().find(|(t, _)| *t == key.key_type) {
        return if *fp == key.fingerprint {
            HostKeyStatus::Trusted
        } else {
            HostKeyStatus::Changed {
                old_key_type: key.key_type.clone(),
                old: fp.clone(),
                new: key.fingerprint.clone(),
            }
        };
    }
    match known.first() {
        Some((t, fp)) => HostKeyStatus::Changed {
            old_key_type: t.clone(),
            old: fp.clone(),
            new: key.fingerprint.clone(),
        },
        None => HostKeyStatus::Unknown {
            fingerprint: key.fingerprint.clone(),
        },
    }
}

/// In-memory trust store.
#[derive(Debug, Default)]
pub struct MemoryVerifier {
    inner: Mutex<MemoryState>,
}

#[derive(Debug, Default)]
struct MemoryState {
    /// host_port → key_type → fingerprint
    keys: HashMap<String, HashMap<String, String>>,
    revoked: HashSet<String>,
}

impl MemoryVerifier {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust `fingerprint` for `(host_port, key_type)`, replacing any previous key.
    pub fn insert(&self, host_port: &str, key_type: &str, fingerprint: &str) {
        self.lock()
            .keys
            .entry(host_port.to_owned())
            .or_default()
            .insert(key_type.to_owned(), fingerprint.to_owned());
    }

    /// Mark a fingerprint as revoked for every host.
    pub fn revoke(&self, fingerprint: &str) {
        self.lock().revoked.insert(fingerprint.to_owned());
    }

    /// Trusted fingerprint for `(host_port, key_type)`.
    pub fn get(&self, host_port: &str, key_type: &str) -> Option<String> {
        self.lock()
            .keys
            .get(host_port)
            .and_then(|m| m.get(key_type))
            .cloned()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        // A panic while holding this lock cannot leave the maps half-updated
        // (single insert calls), so recovering the guard is sound.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl HostKeyVerifier for MemoryVerifier {
    fn verify<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<HostKeyStatus>> {
        let st = self.lock();
        let known: Vec<(String, String)> = st
            .keys
            .get(&key.host_port)
            .map(|m| {
                let mut v: Vec<_> = m.iter().map(|(t, f)| (t.clone(), f.clone())).collect();
                v.sort();
                v
            })
            .unwrap_or_default();
        let status = classify(key, &known, st.revoked.contains(&key.fingerprint));
        drop(st);
        Box::pin(async move { Ok(status) })
    }

    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<()>> {
        self.insert(&key.host_port, &key.key_type, &key.fingerprint);
        Box::pin(async { Ok(()) })
    }
}

/// Trust store backed by the encrypted vault's known-hosts table.
///
/// `VaultStore` is synchronous; every call runs on the blocking pool and
/// holds the mutex only for the duration of one query.
#[derive(Clone)]
pub struct VaultVerifier {
    store: Arc<Mutex<VaultStore>>,
    revoked: Arc<HashSet<String>>,
}

impl VaultVerifier {
    /// Wrap a shared vault store.
    pub fn new(store: Arc<Mutex<VaultStore>>) -> Self {
        Self {
            store,
            revoked: Arc::new(HashSet::new()),
        }
    }

    /// Treat these fingerprints (`SHA256:...`) as revoked, e.g. from
    /// imported `@revoked` known_hosts lines.
    pub fn with_revoked(mut self, fingerprints: impl IntoIterator<Item = String>) -> Self {
        self.revoked = Arc::new(fingerprints.into_iter().collect());
        self
    }

    async fn with_store<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut VaultStore) -> termax_vault::Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = Arc::clone(&self.store);
        tokio::task::spawn_blocking(move || {
            let mut guard = store
                .lock()
                .map_err(|_| Error::HostKeyStore("vault lock poisoned".into()))?;
            f(&mut guard).map_err(|e| Error::HostKeyStore(e.to_string()))
        })
        .await
        .map_err(|e| Error::HostKeyStore(format!("vault task failed: {e}")))?
    }
}

impl HostKeyVerifier for VaultVerifier {
    fn verify<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<HostKeyStatus>> {
        Box::pin(async move {
            let revoked = self.revoked.contains(&key.fingerprint);
            let (hp, kt) = (key.host_port.clone(), key.key_type.clone());
            let known = self
                .with_store(move |s| {
                    if let Some(k) = s.get_known_host(&hp, &kt)? {
                        return Ok(vec![(k.key_type, k.fp_sha256)]);
                    }
                    Ok(s.list_known_hosts()?
                        .into_iter()
                        .filter(|k| k.host_port == hp)
                        .map(|k| (k.key_type, k.fp_sha256))
                        .collect())
                })
                .await?;
            Ok(classify(key, &known, revoked))
        })
    }

    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let entry = KnownHost::new(&key.host_port, &key.key_type, &key.fingerprint, "tofu");
            self.with_store(move |s| s.upsert_known_host(&entry).map(drop))
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kt: &str, fp: &str) -> HostKeyInfo {
        HostKeyInfo {
            host_port: "h:22".into(),
            key_type: kt.into(),
            fingerprint: fp.into(),
        }
    }

    fn pair(t: &str, f: &str) -> (String, String) {
        (t.into(), f.into())
    }

    #[test]
    fn classify_covers_every_status() {
        let k = info("ssh-ed25519", "SHA256:a");
        assert_eq!(
            classify(&k, &[], false),
            HostKeyStatus::Unknown {
                fingerprint: "SHA256:a".into()
            }
        );
        assert_eq!(
            classify(&k, &[pair("ssh-ed25519", "SHA256:a")], false),
            HostKeyStatus::Trusted
        );
        assert_eq!(
            classify(&k, &[pair("ssh-ed25519", "SHA256:b")], false),
            HostKeyStatus::Changed {
                old_key_type: "ssh-ed25519".into(),
                old: "SHA256:b".into(),
                new: "SHA256:a".into()
            }
        );
        // Revocation wins even over a trusted entry.
        assert_eq!(
            classify(&k, &[pair("ssh-ed25519", "SHA256:a")], true),
            HostKeyStatus::Revoked
        );
    }

    #[test]
    fn other_key_type_known_is_a_change_not_unknown() {
        let k = info("ecdsa-sha2-nistp256", "SHA256:new");
        assert!(matches!(
            classify(&k, &[pair("ssh-ed25519", "SHA256:old")], false),
            HostKeyStatus::Changed { old_key_type, .. } if old_key_type == "ssh-ed25519"
        ));
    }

    #[test]
    fn host_port_format_matches_vault() {
        assert_eq!(host_port("DB.Internal", 22), "db.internal:22");
        assert_eq!(host_port("::1", 2222), "[::1]:2222");
        assert_eq!(host_port("127.0.0.1", 2200), "127.0.0.1:2200");
    }

    #[tokio::test]
    async fn memory_verifier_round_trip() {
        let v = MemoryVerifier::new();
        let k = info("ssh-ed25519", "SHA256:a");
        assert!(matches!(
            v.verify(&k).await.unwrap(),
            HostKeyStatus::Unknown { .. }
        ));
        v.save(&k).await.unwrap();
        assert_eq!(v.verify(&k).await.unwrap(), HostKeyStatus::Trusted);
        v.revoke("SHA256:a");
        assert_eq!(v.verify(&k).await.unwrap(), HostKeyStatus::Revoked);
    }

    #[tokio::test]
    async fn vault_verifier_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let sk = termax_vault::generate_secret_key().unwrap();
        let store = VaultStore::create_with_params(
            dir.path().join("v.db"),
            "pw",
            &sk,
            termax_vault::KdfParams::new(8, 1, 1).unwrap(),
        )
        .unwrap();
        let v = VaultVerifier::new(Arc::new(Mutex::new(store)));
        let k = info("ssh-ed25519", "SHA256:a");
        assert!(matches!(
            v.verify(&k).await.unwrap(),
            HostKeyStatus::Unknown { .. }
        ));
        v.save(&k).await.unwrap();
        assert_eq!(v.verify(&k).await.unwrap(), HostKeyStatus::Trusted);
        let changed = info("ssh-ed25519", "SHA256:b");
        assert!(matches!(
            v.verify(&changed).await.unwrap(),
            HostKeyStatus::Changed { .. }
        ));
        let other_type = info("ssh-rsa", "SHA256:c");
        assert!(matches!(
            v.verify(&other_type).await.unwrap(),
            HostKeyStatus::Changed { .. }
        ));
        let v = v.with_revoked(["SHA256:a".to_owned()]);
        assert_eq!(v.verify(&k).await.unwrap(), HostKeyStatus::Revoked);
    }
}
