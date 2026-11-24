//! OpenSSH `known_hosts` parsing, host matching and key fingerprints.
//!
//! Supported line forms (see sshd(8) "SSH_KNOWN_HOSTS FILE FORMAT"):
//!
//! ```text
//! host1,host2,10.0.0.1 ssh-ed25519 AAAA... optional comment
//! [host]:2222 ssh-ed25519 AAAA...
//! |1|<base64 salt>|<base64 HMAC-SHA1> ssh-ed25519 AAAA...
//! @cert-authority *.example.com ssh-ed25519 AAAA...
//! @revoked * ssh-ed25519 AAAA...
//! ```
//!
//! Blank lines and `#` comments are skipped; malformed lines become
//! warnings.

use std::fs;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::error::InteropError;
use crate::pattern::{ListMatch, match_list};
use crate::warning::Warning;

/// SHA-1 output size; hashed host names use a salt of the same length.
const SHA1_LEN: usize = 20;

/// Line marker preceding the host field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// `@cert-authority`: the key is a CA trusted to sign host certificates.
    CertAuthority,
    /// `@revoked`: the key must never be accepted.
    Revoked,
}

/// The host field of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSpec {
    /// Comma-separated patterns (`*`, `?` wildcards, `!` negation,
    /// `[host]:port` for non-default ports).
    Patterns(Vec<String>),
    /// `|1|salt|hash` where hash = HMAC-SHA1(key = salt, host name).
    Hashed { salt: Vec<u8>, hash: Vec<u8> },
}

/// One key line of a known_hosts file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownHostEntry {
    /// 1-based source line.
    pub line: usize,
    pub marker: Option<Marker>,
    pub hosts: HostSpec,
    /// Key type, e.g. `ssh-ed25519`.
    pub key_type: String,
    /// Base64 key as written in the file.
    pub key_base64: String,
    /// Decoded SSH wire-format public key blob.
    pub key_blob: Vec<u8>,
    pub comment: Option<String>,
}

impl KnownHostEntry {
    /// Whether this entry applies to `hostname` on `port`. Port 22 looks up
    /// the bare name, any other port looks up `[hostname]:port`, as ssh does.
    pub fn matches(&self, hostname: &str, port: u16) -> bool {
        let name = lookup_name(hostname, port);
        match &self.hosts {
            HostSpec::Patterns(patterns) => {
                match_list(&name, patterns.iter().map(String::as_str), true) == ListMatch::Matched
            }
            HostSpec::Hashed { salt, hash } => {
                let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(salt) else {
                    return false;
                };
                mac.update(name.as_bytes());
                mac.verify_slice(hash).is_ok()
            }
        }
    }

    /// OpenSSH fingerprint of the key: `SHA256:<base64 without padding>`.
    pub fn fingerprint_sha256(&self) -> String {
        fingerprint_sha256(&self.key_blob)
    }

    pub fn is_hashed(&self) -> bool {
        matches!(self.hosts, HostSpec::Hashed { .. })
    }
}

/// OpenSSH SHA256 fingerprint (`SHA256:...`) of a public key blob.
pub fn fingerprint_sha256(key_blob: &[u8]) -> String {
    format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(key_blob))
    )
}

/// Host name as stored in known_hosts for `hostname`/`port`.
fn lookup_name(hostname: &str, port: u16) -> String {
    let host = hostname.to_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// A parsed known_hosts file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownHosts {
    pub entries: Vec<KnownHostEntry>,
    pub warnings: Vec<Warning>,
}

impl KnownHosts {
    /// Parse known_hosts text; bad lines are reported in `warnings`.
    pub fn parse_str(input: &str) -> Self {
        Self::parse(input, None)
    }

    /// Read and parse the known_hosts file at `path`.
    pub fn parse_file(path: &Path) -> Result<Self, InteropError> {
        let text = fs::read_to_string(path).map_err(|source| InteropError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(Self::parse(&text, Some(path)))
    }

    fn parse(input: &str, file: Option<&Path>) -> Self {
        let mut out = Self::default();
        for (idx, raw) in input.lines().enumerate() {
            let line = idx + 1;
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match parse_line(trimmed, line) {
                Ok(entry) => out.entries.push(entry),
                Err(msg) => out.warnings.push(Warning::new(file, Some(line), msg)),
            }
        }
        out
    }

    /// Entries that apply to `hostname` on `port`, in file order (including
    /// `@revoked` and `@cert-authority` entries; check [`KnownHostEntry::marker`]).
    pub fn lookup<'a>(
        &'a self,
        hostname: &'a str,
        port: u16,
    ) -> impl Iterator<Item = &'a KnownHostEntry> + 'a {
        self.entries
            .iter()
            .filter(move |e| e.matches(hostname, port))
    }
}

fn parse_line(line: &str, line_no: usize) -> Result<KnownHostEntry, String> {
    let mut fields = line.split_ascii_whitespace();
    let mut first = fields.next().ok_or("empty line")?;
    let marker = match first {
        "@cert-authority" => Some(Marker::CertAuthority),
        "@revoked" => Some(Marker::Revoked),
        m if m.starts_with('@') => return Err(format!("unknown marker `{m}`; line ignored")),
        _ => None,
    };
    if marker.is_some() {
        first = fields.next().ok_or("missing host field after marker")?;
    }
    let hosts = parse_hosts(first)?;
    let key_type = fields.next().ok_or("missing key type")?.to_owned();
    let key_base64 = fields.next().ok_or("missing key data")?.to_owned();
    let comment = {
        let rest: Vec<&str> = fields.collect();
        (!rest.is_empty()).then(|| rest.join(" "))
    };
    let key_blob = STANDARD
        .decode(&key_base64)
        .map_err(|e| format!("invalid base64 key data: {e}"))?;
    let blob_type = blob_key_type(&key_blob).ok_or("key data is not an SSH public key blob")?;
    if blob_type != key_type {
        return Err(format!(
            "key type `{key_type}` does not match key data type `{blob_type}`"
        ));
    }
    Ok(KnownHostEntry {
        line: line_no,
        marker,
        hosts,
        key_type,
        key_base64,
        key_blob,
        comment,
    })
}

fn parse_hosts(field: &str) -> Result<HostSpec, String> {
    if let Some(rest) = field.strip_prefix("|1|") {
        let (salt, hash) = rest
            .split_once('|')
            .ok_or("hashed host must be `|1|salt|hash`")?;
        let salt = STANDARD
            .decode(salt)
            .map_err(|e| format!("invalid hashed host salt: {e}"))?;
        let hash = STANDARD
            .decode(hash)
            .map_err(|e| format!("invalid hashed host hash: {e}"))?;
        if salt.len() != SHA1_LEN || hash.len() != SHA1_LEN {
            return Err("hashed host salt and hash must be 20 bytes".into());
        }
        return Ok(HostSpec::Hashed { salt, hash });
    }
    if field.starts_with('|') {
        return Err("unsupported hashed host format".into());
    }
    let patterns: Vec<String> = field
        .split(',')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    if patterns.is_empty() {
        return Err("empty host field".into());
    }
    Ok(HostSpec::Patterns(patterns))
}

/// Key type string embedded at the start of an SSH public key blob.
fn blob_key_type(blob: &[u8]) -> Option<&str> {
    let len = u32::from_be_bytes(blob.get(..4)?.try_into().ok()?) as usize;
    let end = 4usize.checked_add(len)?;
    std::str::from_utf8(blob.get(4..end)?).ok()
}
