//! Server interoperability quirks, keyed by the server's version string.
//!
//! # Table format
//!
//! [`QUIRKS`] is a static list of [`QuirkEntry`] rows. Each row has:
//! - `pattern`: a regular expression (`regex-lite` syntax) matched against
//!   the server identification line without the trailing CR LF, e.g.
//!   `SSH-2.0-OpenSSH_7.1p2 Debian-2`. Anchor it with `^`.
//! - `quirks`: the [`Quirks`] flags to switch on.
//! - `note`: why the row exists (shown in debug logs).
//!
//! Every matching row is applied (flags are OR-ed). Rows describe protocol
//! facts about a server family only; they can never weaken the crypto policy
//! or host-key checking. The server version is learned during the handshake,
//! so quirks only affect behaviour after key exchange (authentication).

use std::sync::LazyLock;

use regex_lite::Regex;

/// Behaviour switches derived from the server version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Quirks {
    /// The server cannot verify `rsa-sha2-256/512` signatures. RSA keys are
    /// skipped (we never fall back to SHA-1 `ssh-rsa`).
    pub no_rsa_sha2: bool,
}

impl Quirks {
    fn merge(self, other: Quirks) -> Quirks {
        Quirks {
            no_rsa_sha2: self.no_rsa_sha2 || other.no_rsa_sha2,
        }
    }
}

/// One row of the quirks table.
#[derive(Debug, Clone, Copy)]
pub struct QuirkEntry {
    /// Regex over the server identification string.
    pub pattern: &'static str,
    /// Flags applied when the pattern matches.
    pub quirks: Quirks,
    /// Rationale.
    pub note: &'static str,
}

/// The quirks table. Keep it small and factual.
pub const QUIRKS: &[QuirkEntry] = &[
    QuirkEntry {
        pattern: r"^SSH-2\.0-OpenSSH_([1-6]\.|7\.[01]([^0-9]|$))",
        quirks: Quirks { no_rsa_sha2: true },
        note: "OpenSSH before 7.2 predates RFC 8332 rsa-sha2 signatures",
    },
    QuirkEntry {
        pattern: r"^SSH-2\.0-dropbear_(0\.|20(0\d|1\d)\.|2020\.([0-6]\d|7[0-8])([^0-9]|$))",
        quirks: Quirks { no_rsa_sha2: true },
        note: "Dropbear before 2020.79 predates rsa-sha2 signatures",
    },
];

static COMPILED: LazyLock<Vec<(Regex, &'static QuirkEntry)>> = LazyLock::new(|| {
    QUIRKS
        .iter()
        .filter_map(|e| match Regex::new(e.pattern) {
            Ok(r) => Some((r, e)),
            Err(err) => {
                tracing::error!(pattern = e.pattern, %err, "invalid quirk pattern ignored");
                None
            }
        })
        .collect()
});

/// Quirks for a server identification string (CR LF is trimmed).
pub fn quirks_for(server_id: &str) -> Quirks {
    let id = server_id.trim_end_matches(['\r', '\n']);
    COMPILED
        .iter()
        .filter(|(re, _)| re.is_match(id))
        .fold(Quirks::default(), |acc, (_, e)| {
            tracing::debug!(server = id, note = e.note, "applying ssh quirk");
            acc.merge(e.quirks)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_compiles() {
        assert_eq!(COMPILED.len(), QUIRKS.len());
    }

    #[test]
    fn old_openssh_lacks_rsa_sha2() {
        for id in [
            "SSH-2.0-OpenSSH_6.6.1p1 Ubuntu-2ubuntu2",
            "SSH-2.0-OpenSSH_7.1p2",
            "SSH-2.0-OpenSSH_7.0",
            "SSH-2.0-OpenSSH_5.3\r\n",
        ] {
            assert!(quirks_for(id).no_rsa_sha2, "{id}");
        }
    }

    #[test]
    fn modern_servers_have_no_quirks() {
        for id in [
            "SSH-2.0-OpenSSH_7.2p2 Ubuntu-4",
            "SSH-2.0-OpenSSH_7.10",
            "SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.8",
            "SSH-2.0-dropbear_2020.79",
            "SSH-2.0-dropbear_2022.83",
            "SSH-2.0-russh_0.65",
        ] {
            assert_eq!(quirks_for(id), Quirks::default(), "{id}");
        }
    }

    #[test]
    fn old_dropbear_lacks_rsa_sha2() {
        for id in [
            "SSH-2.0-dropbear_2019.78",
            "SSH-2.0-dropbear_2020.78",
            "SSH-2.0-dropbear_0.52",
        ] {
            assert!(quirks_for(id).no_rsa_sha2, "{id}");
        }
    }
}
