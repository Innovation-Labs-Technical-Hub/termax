//! Append-only, SHA-256 hash-chained audit log of policy decisions.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::engine::Actor;
use crate::error::PolicyError;
use crate::rules::{Effect, Verdict};

/// `prev_hash` of the first entry.
pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Position in the log, starting at 0.
    pub seq: u64,
    /// Unix time in milliseconds.
    pub timestamp_ms: u64,
    /// Who proposed the command.
    pub actor: Actor,
    /// The command as proposed.
    pub command: String,
    /// The decided effect.
    pub verdict: Effect,
    /// Reasons for the decision.
    pub reasons: Vec<String>,
    /// Hex SHA-256 of the previous entry (or [`GENESIS_HASH`]).
    pub prev_hash: String,
    /// Hex SHA-256 over `prev_hash` followed by the canonical entry bytes.
    pub hash: String,
}

/// The hashed fields, in a fixed order. `serde_json` serialises struct
/// fields in declaration order, which makes these bytes canonical.
#[derive(Serialize)]
struct Canonical<'a> {
    seq: u64,
    timestamp_ms: u64,
    actor: &'a Actor,
    command: &'a str,
    verdict: Effect,
    reasons: &'a [String],
    prev_hash: &'a str,
}

impl AuditEntry {
    /// Recompute this entry's hash from its fields.
    pub fn compute_hash(&self) -> String {
        let canonical = Canonical {
            seq: self.seq,
            timestamp_ms: self.timestamp_ms,
            actor: &self.actor,
            command: &self.command,
            verdict: self.verdict,
            reasons: &self.reasons,
            prev_hash: &self.prev_hash,
        };
        // Serialising plain strings, integers and enums cannot fail.
        let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(self.prev_hash.as_bytes());
        hasher.update(&bytes);
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Why [`AuditLog::verify`] rejected a log.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AuditError {
    /// Entry at `index` has an unexpected sequence number (reordering,
    /// insertion or deletion).
    #[error("entry {index}: expected seq {index}, found {found}")]
    Sequence {
        /// Position in the log.
        index: usize,
        /// The seq stored in the entry.
        found: u64,
    },
    /// Entry's `prev_hash` does not match the previous entry's hash.
    #[error("entry {index}: broken hash chain")]
    BrokenChain {
        /// Position in the log.
        index: usize,
    },
    /// Entry's stored hash does not match its contents.
    #[error("entry {index}: hash does not match contents")]
    HashMismatch {
        /// Position in the log.
        index: usize,
    },
}

/// In-memory append-only audit log. Entries can only be added through
/// [`AuditLog::append`], which links each entry to its predecessor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
}

impl AuditLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a decision and return the new entry.
    pub fn append(
        &mut self,
        timestamp_ms: u64,
        actor: Actor,
        command: &str,
        verdict: &Verdict,
    ) -> &AuditEntry {
        let mut entry = AuditEntry {
            seq: self.entries.len() as u64,
            timestamp_ms,
            actor,
            command: command.to_string(),
            verdict: verdict.effect,
            reasons: verdict.reasons.clone(),
            prev_hash: self.head_hash().to_string(),
            hash: String::new(),
        };
        entry.hash = entry.compute_hash();
        self.entries.push(entry);
        &self.entries[self.entries.len() - 1]
    }

    /// All entries in order.
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no entries have been recorded.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Hash of the newest entry, or [`GENESIS_HASH`]. Store it separately
    /// (anchor it) to also detect truncation of the tail.
    pub fn head_hash(&self) -> &str {
        self.entries
            .last()
            .map_or(GENESIS_HASH, |e| e.hash.as_str())
    }

    /// Check sequence numbers, chain links and per-entry hashes.
    pub fn verify(&self) -> Result<(), AuditError> {
        let mut prev = GENESIS_HASH;
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.seq != index as u64 {
                return Err(AuditError::Sequence {
                    index,
                    found: entry.seq,
                });
            }
            if entry.prev_hash != prev {
                return Err(AuditError::BrokenChain { index });
            }
            if entry.compute_hash() != entry.hash {
                return Err(AuditError::HashMismatch { index });
            }
            prev = &entry.hash;
        }
        Ok(())
    }

    /// Serialise as JSON lines, one entry per line.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            // Entries only contain strings, integers and enums.
            out.push_str(&serde_json::to_string(entry).unwrap_or_default());
            out.push('\n');
        }
        out
    }

    /// Decode JSON lines produced by [`AuditLog::to_jsonl`]. Blank lines are
    /// ignored. The result is not verified; call [`AuditLog::verify`].
    pub fn from_jsonl(src: &str) -> Result<Self, PolicyError> {
        let entries = src
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| {
                serde_json::from_str(l).map_err(|source| PolicyError::AuditDecode {
                    line: i + 1,
                    source,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AuditLog {
        let mut log = AuditLog::new();
        log.append(1, Actor::InApp, "ls", &Verdict::new(Effect::Allow, "ok"));
        log.append(
            2,
            Actor::External {
                client_id: "claude".into(),
            },
            "rm -rf /",
            &Verdict::new(Effect::Deny, "nope"),
        );
        log.append(
            3,
            Actor::InApp,
            "reboot",
            &Verdict::new(Effect::Ask, "power"),
        );
        log
    }

    #[test]
    fn valid_chain_verifies_and_round_trips() {
        let log = sample();
        assert_eq!(log.len(), 3);
        assert_eq!(log.entries()[0].prev_hash, GENESIS_HASH);
        assert_eq!(log.entries()[1].prev_hash, log.entries()[0].hash);
        assert_eq!(log.verify(), Ok(()));
        let text = log.to_jsonl();
        assert_eq!(text.lines().count(), 3);
        let back = AuditLog::from_jsonl(&text).unwrap();
        assert_eq!(back, log);
        assert_eq!(back.verify(), Ok(()));
    }

    /// Verifies: FR-AG03-010 acceptance 1
    #[test]
    fn detects_modified_content() {
        let mut log = sample();
        log.entries[1].verdict = Effect::Allow;
        assert_eq!(log.verify(), Err(AuditError::HashMismatch { index: 1 }));
    }

    #[test]
    fn detects_rehashed_entry_breaking_the_chain() {
        let mut log = sample();
        log.entries[0].command = "rm -rf /".into();
        log.entries[0].hash = log.entries[0].compute_hash();
        assert_eq!(log.verify(), Err(AuditError::BrokenChain { index: 1 }));
    }

    #[test]
    fn detects_reordering_and_deletion() {
        let mut log = sample();
        log.entries.swap(1, 2);
        assert_eq!(
            log.verify(),
            Err(AuditError::Sequence { index: 1, found: 2 })
        );
        let mut log = sample();
        log.entries.remove(0);
        assert_eq!(
            log.verify(),
            Err(AuditError::Sequence { index: 0, found: 1 })
        );
    }

    /// Verifies: FR-AG03-010 acceptance 1
    #[test]
    fn detects_tampering_in_serialised_form() {
        let text = sample().to_jsonl().replace("\"deny\"", "\"allow\"");
        let log = AuditLog::from_jsonl(&text).unwrap();
        assert_eq!(log.verify(), Err(AuditError::HashMismatch { index: 1 }));
        assert!(matches!(
            AuditLog::from_jsonl("{not json}"),
            Err(PolicyError::AuditDecode { line: 1, .. })
        ));
    }
}
