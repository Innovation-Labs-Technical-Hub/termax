//! Trusted host keys (TOFU store).
//!
//! The primary key is `(host_port, key_type)`. Because `host_port` names a
//! host, it is stored as a keyed HMAC token (`host_port_mac`, like OpenSSH's
//! hashed known_hosts) and the clear value, fingerprint and source are kept in
//! an encrypted record. `key_type` and `first_seen` are plaintext.

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::crypto::{Aad, mac_token, open_json, seal_json};
use crate::error::{Error, Result};
use crate::store::VaultStore;

/// A trusted host key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHost {
    /// Host and port as used for lookup, e.g. `db.internal:22` (matched exactly).
    pub host_port: String,
    /// SSH key algorithm, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// SHA-256 fingerprint, e.g. `SHA256:...`.
    pub fp_sha256: String,
    /// Where the key came from (`tofu`, `import:~/.ssh/known_hosts`, ...).
    pub source: String,
    /// Unix seconds when the key was first trusted.
    pub first_seen: i64,
}

impl KnownHost {
    /// New entry with `first_seen` set to now.
    pub fn new(
        host_port: impl Into<String>,
        key_type: impl Into<String>,
        fp_sha256: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        Self {
            host_port: host_port.into(),
            key_type: key_type.into(),
            fp_sha256: fp_sha256.into(),
            source: source.into(),
            first_seen: now,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Record {
    host_port: String,
    fp_sha256: String,
    source: String,
}

fn record_aad(mac: &str, key_type: &str, first_seen: i64) -> Aad {
    Aad::new("known_hosts", "record_ct")
        .field(mac)
        .field(key_type)
        .field(first_seen.to_be_bytes())
}

impl VaultStore {
    fn host_port_mac(&self, host_port: &str) -> Result<String> {
        mac_token(&self.kh_mac_key, host_port.as_bytes())
    }

    fn decode_known_host(
        &self,
        mac: &str,
        key_type: String,
        first_seen: i64,
        ct: &[u8],
    ) -> Result<KnownHost> {
        let rec: Record = open_json(
            &self.kh_enc_key,
            ct,
            &record_aad(mac, &key_type, first_seen),
            "known host",
        )?;
        if self.host_port_mac(&rec.host_port)? != mac {
            return Err(Error::Corrupt("known host: index mismatch".into()));
        }
        Ok(KnownHost {
            host_port: rec.host_port,
            key_type,
            fp_sha256: rec.fp_sha256,
            source: rec.source,
            first_seen,
        })
    }

    /// Insert or replace the key for `(host_port, key_type)`. An existing
    /// entry keeps its original `first_seen`; otherwise `entry.first_seen` is
    /// used. Returns the stored entry.
    pub fn upsert_known_host(&mut self, entry: &KnownHost) -> Result<KnownHost> {
        if entry.host_port.is_empty() || entry.key_type.is_empty() {
            return Err(Error::InvalidInput(
                "known host needs host_port and key_type".into(),
            ));
        }
        let mac = self.host_port_mac(&entry.host_port)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT first_seen FROM known_hosts WHERE host_port_mac = ?1 AND key_type = ?2",
                params![mac, entry.key_type],
                |r| r.get(0),
            )
            .optional()?;
        let first_seen = existing.unwrap_or(entry.first_seen);
        let ct = seal_json(
            &self.kh_enc_key,
            &Record {
                host_port: entry.host_port.clone(),
                fp_sha256: entry.fp_sha256.clone(),
                source: entry.source.clone(),
            },
            &record_aad(&mac, &entry.key_type, first_seen),
        )?;
        tx.execute(
            "INSERT INTO known_hosts (host_port_mac, key_type, first_seen, record_ct) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (host_port_mac, key_type) DO UPDATE SET record_ct = excluded.record_ct",
            params![mac, entry.key_type, first_seen, ct],
        )?;
        tx.commit()?;
        Ok(KnownHost {
            first_seen,
            ..entry.clone()
        })
    }

    /// Look up the trusted key for `(host_port, key_type)`.
    pub fn get_known_host(&self, host_port: &str, key_type: &str) -> Result<Option<KnownHost>> {
        let mac = self.host_port_mac(host_port)?;
        let row: Option<(i64, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT first_seen, record_ct FROM known_hosts \
                 WHERE host_port_mac = ?1 AND key_type = ?2",
                params![mac, key_type],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(first_seen, ct)| {
            self.decode_known_host(&mac, key_type.to_owned(), first_seen, &ct)
        })
        .transpose()
    }

    /// All trusted keys, ordered by host then key type.
    pub fn list_known_hosts(&self) -> Result<Vec<KnownHost>> {
        let rows: Vec<(String, String, i64, Vec<u8>)> = {
            let mut stmt = self.conn.prepare(
                "SELECT host_port_mac, key_type, first_seen, record_ct FROM known_hosts",
            )?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut out = rows
            .into_iter()
            .map(|(mac, kt, fs, ct)| self.decode_known_host(&mac, kt, fs, &ct))
            .collect::<Result<Vec<_>>>()?;
        out.sort_by(|a, b| {
            (a.host_port.as_str(), a.key_type.as_str())
                .cmp(&(b.host_port.as_str(), b.key_type.as_str()))
        });
        Ok(out)
    }
}
