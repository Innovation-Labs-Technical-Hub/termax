//! Paired clients and their scopes. The registry stores token hashes only.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::ControlError;
use crate::token::Token;

/// What a client may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// List sessions, subscribe to events, mirror output.
    Observe,
    /// Also open sessions, send input and specials, attach interactively.
    Operate,
}

/// Whether `scopes` allow `needed` (operate includes observe).
pub fn allows(scopes: &[Scope], needed: Scope) -> bool {
    scopes.contains(&needed) || (needed == Scope::Observe && scopes.contains(&Scope::Operate))
}

/// Kind of external client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// The `tmx` CLI.
    Cli,
    /// An MCP shim (`tmx mcp`): an AI agent, untrusted; output is sanitised.
    Mcp,
}

/// A paired client (the `cli_clients` entity; the token itself stays in
/// the client's keychain).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRecord {
    /// Client id (UUIDv4).
    pub id: String,
    /// Display name given at pairing.
    pub name: String,
    /// Kind.
    pub kind: ClientKind,
    /// Executable of the process that paired, when known.
    pub binary_path: Option<String>,
    /// Granted scopes.
    pub scopes: Vec<Scope>,
    /// Hex SHA-256 of the token.
    pub token_sha256: String,
    /// Unix seconds.
    pub paired_at: u64,
    /// Unix seconds of the last successful `control.hello`.
    pub last_seen_at: Option<u64>,
    /// Revoked clients cannot authenticate.
    pub revoked: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    clients: Vec<ClientRecord>,
}

/// Why authentication failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailure {
    /// No client has this token.
    Unknown,
    /// The client was revoked.
    Revoked,
}

/// The paired clients, persisted as JSON (mode 0600) or kept in memory.
#[derive(Debug)]
pub struct ClientRegistry {
    path: Option<PathBuf>,
    records: Mutex<Vec<ClientRecord>>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl ClientRegistry {
    /// A registry that is not persisted (tests, embedded use).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            records: Mutex::default(),
        }
    }

    /// Load the registry at `path` (empty when the file does not exist).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, ControlError> {
        let path = path.into();
        let err = |reason: String| ControlError::Registry {
            path: path.display().to_string(),
            reason,
        };
        let records = match std::fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<RegistryFile>(&bytes)
                    .map_err(|e| err(e.to_string()))?
                    .clients
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(err(e.to_string())),
        };
        Ok(Self {
            path: Some(path),
            records: Mutex::new(records),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Vec<ClientRecord>> {
        self.records.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Every client, revoked ones included.
    pub fn list(&self) -> Vec<ClientRecord> {
        self.lock().clone()
    }

    /// Register a newly approved client and return its record and token.
    pub fn issue(
        &self,
        name: &str,
        kind: ClientKind,
        binary_path: Option<String>,
        scopes: Vec<Scope>,
    ) -> Result<(ClientRecord, Token), ControlError> {
        let token = Token::generate()?;
        let record = ClientRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_owned(),
            kind,
            binary_path,
            scopes,
            token_sha256: token.hash(),
            paired_at: now_secs(),
            last_seen_at: None,
            revoked: false,
        };
        let mut records = self.lock();
        records.push(record.clone());
        self.save(&records)?;
        Ok((record, token))
    }

    /// The client holding `token`, which is then marked as seen.
    pub fn authenticate(&self, token: &Token) -> Result<ClientRecord, AuthFailure> {
        let mut records = self.lock();
        let rec = records
            .iter_mut()
            .find(|r| token.matches(&r.token_sha256))
            .ok_or(AuthFailure::Unknown)?;
        if rec.revoked {
            return Err(AuthFailure::Revoked);
        }
        rec.last_seen_at = Some(now_secs());
        let rec = rec.clone();
        if let Err(e) = self.save(&records) {
            tracing::warn!(error = %e, "client registry not updated");
        }
        Ok(rec)
    }

    /// Whether `id` is still a valid (not revoked) client.
    pub fn is_active(&self, id: &str) -> bool {
        self.lock().iter().any(|r| r.id == id && !r.revoked)
    }

    /// Revoke a client. Returns whether it existed.
    pub fn revoke(&self, id: &str) -> Result<bool, ControlError> {
        let mut records = self.lock();
        let Some(rec) = records.iter_mut().find(|r| r.id == id) else {
            return Ok(false);
        };
        rec.revoked = true;
        self.save(&records)?;
        Ok(true)
    }

    fn save(&self, records: &[ClientRecord]) -> Result<(), ControlError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let err = |reason: String| ControlError::Registry {
            path: path.display().to_string(),
            reason,
        };
        let body = serde_json::to_vec_pretty(&RegistryFile {
            clients: records.to_vec(),
        })
        .map_err(|e| err(e.to_string()))?;
        write_private(path, &body).map_err(|e| err(e.to_string()))
    }
}

/// Write `body` to `path` through a 0600 temporary file and a rename.
fn write_private(path: &Path, body: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    f.write_all(body)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operate_includes_observe_but_not_the_reverse() {
        assert!(allows(&[Scope::Operate], Scope::Observe));
        assert!(allows(&[Scope::Observe], Scope::Observe));
        assert!(!allows(&[Scope::Observe], Scope::Operate));
        assert!(!allows(&[], Scope::Observe));
    }

    #[test]
    fn registry_persists_hashes_only_with_private_mode_and_honours_revocation() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/control-clients.json");
        let reg = ClientRegistry::open(&path).unwrap();
        let (rec, token) = reg
            .issue(
                "tmx",
                ClientKind::Cli,
                Some("/usr/bin/tmx".into()),
                vec![Scope::Observe],
            )
            .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains(token.expose()), "token stored in clear");
        assert!(raw.contains(&token.hash()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let reopened = ClientRegistry::open(&path).unwrap();
        assert_eq!(reopened.authenticate(&token).unwrap().id, rec.id);
        assert_eq!(
            reopened.authenticate(&Token::generate().unwrap()),
            Err(AuthFailure::Unknown)
        );
        assert!(reopened.revoke(&rec.id).unwrap());
        assert!(!reopened.is_active(&rec.id));
        assert_eq!(reopened.authenticate(&token), Err(AuthFailure::Revoked));
        assert_eq!(
            ClientRegistry::open(&path).unwrap().authenticate(&token),
            Err(AuthFailure::Revoked)
        );
        assert!(!reopened.revoke("nope").unwrap());
    }

    #[test]
    fn a_corrupt_registry_is_an_error_not_an_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("c.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(matches!(
            ClientRegistry::open(&path),
            Err(ControlError::Registry { .. })
        ));
    }
}
