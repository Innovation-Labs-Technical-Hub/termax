//! Connection options and the crypto policy.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use russh::keys::Algorithm;
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// Smallest accepted initial channel window (2 MiB).
pub const MIN_WINDOW_SIZE: u32 = 2 * 1024 * 1024;
/// Smallest accepted maximum channel packet size (32 KiB).
pub const MIN_PACKET_SIZE: u32 = 32 * 1024;
/// Largest accepted maximum channel packet size (256 KiB).
pub const MAX_PACKET_SIZE: u32 = 256 * 1024;

/// Everything needed to open one SSH connection.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Target host name or address.
    pub host: String,
    /// Target port.
    pub port: u16,
    /// Remote user name.
    pub user: String,
    /// Authentication methods, tried in order.
    pub auth: Vec<AuthMethod>,
    /// Budget for TCP connect + handshake + authentication of each hop.
    /// Time spent waiting on user prompts is excluded.
    pub connect_timeout: Duration,
    /// Send `keepalive@openssh.com` (want-reply) after this much silence.
    /// `None` disables keepalives.
    pub keepalive_interval: Option<Duration>,
    /// After this many unanswered keepalives the connection is declared dead
    /// and [`crate::CloseReason::KeepaliveTimeout`] is published. Must be ≥ 1.
    pub keepalive_max_missed: usize,
    /// Jump hosts, outermost first (`ProxyJump a,b` → `[a, b]`).
    pub jump_hosts: Vec<JumpHop>,
    /// Algorithm preferences.
    pub crypto: CryptoPolicy,
    /// Initial channel window advertised to the server (≥ 2 MiB).
    pub window_size: u32,
    /// Maximum channel packet size advertised to the server (32–256 KiB).
    pub max_packet_size: u32,
    /// Accept a server that reports success without verifying a credential
    /// (`none` accepted, or keyboard-interactive with no prompts). Off by
    /// default: such a server could be impersonating the real one to a user
    /// who would then type secrets into it.
    pub allow_trivial_auth: bool,
}

impl ConnectOptions {
    /// Options with secure defaults: port 22, 15 s connect timeout,
    /// 30 s keepalive with 3 misses allowed, 2 MiB window, 256 KiB packets.
    pub fn new(host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: 22,
            user: user.into(),
            auth: Vec::new(),
            connect_timeout: Duration::from_secs(15),
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max_missed: 3,
            jump_hosts: Vec::new(),
            crypto: CryptoPolicy::default(),
            window_size: MIN_WINDOW_SIZE,
            max_packet_size: MAX_PACKET_SIZE,
            allow_trivial_auth: false,
        }
    }

    /// Check ranges and the crypto policy before any I/O.
    pub fn validate(&self) -> Result<()> {
        if self.host.is_empty() {
            return Err(Error::InvalidConfig("host is empty".into()));
        }
        if self.user.is_empty() {
            return Err(Error::InvalidConfig("user is empty".into()));
        }
        if self.port == 0 {
            return Err(Error::InvalidConfig("port 0".into()));
        }
        if self.connect_timeout.is_zero() {
            return Err(Error::InvalidConfig("connect_timeout is zero".into()));
        }
        if self.keepalive_interval.is_some_and(|d| d.is_zero()) {
            return Err(Error::InvalidConfig("keepalive_interval is zero".into()));
        }
        if self.keepalive_max_missed == 0 {
            return Err(Error::InvalidConfig(
                "keepalive_max_missed must be ≥ 1".into(),
            ));
        }
        if self.window_size < MIN_WINDOW_SIZE {
            return Err(Error::InvalidConfig(format!(
                "window_size {} below minimum {MIN_WINDOW_SIZE}",
                self.window_size
            )));
        }
        if !(MIN_PACKET_SIZE..=MAX_PACKET_SIZE).contains(&self.max_packet_size) {
            return Err(Error::InvalidConfig(format!(
                "max_packet_size {} outside {MIN_PACKET_SIZE}..={MAX_PACKET_SIZE}",
                self.max_packet_size
            )));
        }
        for hop in &self.jump_hosts {
            if hop.host.is_empty() || hop.user.is_empty() || hop.port == 0 {
                return Err(Error::InvalidConfig(format!(
                    "jump host `{}@{}:{}` is incomplete",
                    hop.user, hop.host, hop.port
                )));
            }
        }
        self.crypto.to_preferred().map(drop)
    }
}

/// One `ProxyJump` hop. Crypto policy, timeouts and window sizes are taken
/// from the enclosing [`ConnectOptions`].
#[derive(Debug, Clone)]
pub struct JumpHop {
    /// Hop host name or address.
    pub host: String,
    /// Hop port.
    pub port: u16,
    /// User on the hop.
    pub user: String,
    /// Authentication methods for the hop, tried in order.
    pub auth: Vec<AuthMethod>,
}

/// An authentication method. `Debug` never prints secrets.
#[derive(Clone)]
#[non_exhaustive]
pub enum AuthMethod {
    /// OpenSSH (or PuTTY) private key file. When the key is encrypted and no
    /// passphrase is given, [`crate::Prompter::passphrase`] is asked.
    PublicKeyFile {
        path: PathBuf,
        passphrase: Option<Zeroizing<String>>,
    },
    /// Password; `None` asks [`crate::Prompter::password`].
    Password(Option<Zeroizing<String>>),
    /// Keyboard-interactive; prompts go to [`crate::Prompter::keyboard_interactive`].
    KeyboardInteractive,
    /// Every identity held by an ssh-agent. `socket: None` uses
    /// `SSH_AUTH_SOCK` on Unix and the OpenSSH agent pipe on Windows.
    Agent { socket: Option<PathBuf> },
}

impl AuthMethod {
    /// SSH method name on the wire.
    pub fn wire_name(&self) -> &'static str {
        match self {
            AuthMethod::PublicKeyFile { .. } | AuthMethod::Agent { .. } => "publickey",
            AuthMethod::Password(_) => "password",
            AuthMethod::KeyboardInteractive => "keyboard-interactive",
        }
    }
}

impl fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthMethod::PublicKeyFile { path, passphrase } => f
                .debug_struct("PublicKeyFile")
                .field("path", path)
                .field("passphrase", &passphrase.as_ref().map(|_| "<redacted>"))
                .finish(),
            AuthMethod::Password(p) => f
                .debug_tuple("Password")
                .field(&p.as_ref().map(|_| "<redacted>"))
                .finish(),
            AuthMethod::KeyboardInteractive => f.write_str("KeyboardInteractive"),
            AuthMethod::Agent { socket } => {
                f.debug_struct("Agent").field("socket", socket).finish()
            }
        }
    }
}

/// Algorithm preferences, most preferred first, as SSH wire names.
///
/// Strict key exchange (`kex-strict-c-v00@openssh.com`, the Terrapin
/// countermeasure) and `ext-info-c` (needed for `server-sig-algs`) are always
/// appended and cannot be turned off. Compression is always `none`.
/// Legacy algorithms (`ssh-rsa` SHA-1 signatures, SHA-1 key exchanges and
/// MACs, CBC/3DES ciphers, `none`) are rejected by [`CryptoPolicy::to_preferred`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CryptoPolicy {
    /// Key exchange algorithms.
    pub kex: Vec<String>,
    /// Host key signature algorithms.
    pub host_key: Vec<String>,
    /// Ciphers.
    pub cipher: Vec<String>,
    /// MACs (unused with AEAD ciphers).
    pub mac: Vec<String>,
}

/// Default key exchange order. `sntrup761x25519-sha512` is not implemented
/// by the backend, so the only post-quantum hybrid offered is ML-KEM.
pub const DEFAULT_KEX: &[&str] = &[
    "mlkem768x25519-sha256",
    "curve25519-sha256",
    "curve25519-sha256@libssh.org",
    "ecdh-sha2-nistp256",
    "ecdh-sha2-nistp384",
    "ecdh-sha2-nistp521",
    "diffie-hellman-group-exchange-sha256",
    "diffie-hellman-group16-sha512",
    "diffie-hellman-group18-sha512",
    "diffie-hellman-group14-sha256",
];

/// Default host key algorithm order (never `ssh-rsa`/SHA-1).
pub const DEFAULT_HOST_KEY: &[&str] = &[
    "ssh-ed25519",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "rsa-sha2-512",
    "rsa-sha2-256",
];

/// Default cipher order.
pub const DEFAULT_CIPHER: &[&str] = &[
    "chacha20-poly1305@openssh.com",
    "aes256-gcm@openssh.com",
    "aes128-gcm@openssh.com",
    "aes256-ctr",
    "aes192-ctr",
    "aes128-ctr",
];

/// Default MAC order (encrypt-then-MAC first).
pub const DEFAULT_MAC: &[&str] = &[
    "hmac-sha2-512-etm@openssh.com",
    "hmac-sha2-256-etm@openssh.com",
    "hmac-sha2-512",
    "hmac-sha2-256",
];

const FORBIDDEN: &[&str] = &[
    "ssh-rsa",
    "ssh-dss",
    "diffie-hellman-group1-sha1",
    "diffie-hellman-group14-sha1",
    "diffie-hellman-group-exchange-sha1",
    "hmac-sha1",
    "hmac-sha1-etm@openssh.com",
    "3des-cbc",
    "aes128-cbc",
    "aes192-cbc",
    "aes256-cbc",
    "none",
    "clear",
];

const STRICT_KEX_CLIENT: &str = "kex-strict-c-v00@openssh.com";
const EXT_INFO_CLIENT: &str = "ext-info-c";

impl Default for CryptoPolicy {
    fn default() -> Self {
        let own = |l: &[&str]| l.iter().map(|s| (*s).to_owned()).collect();
        Self {
            kex: own(DEFAULT_KEX),
            host_key: own(DEFAULT_HOST_KEY),
            cipher: own(DEFAULT_CIPHER),
            mac: own(DEFAULT_MAC),
        }
    }
}

impl CryptoPolicy {
    /// Translate to the backend's preference lists, rejecting unknown and
    /// forbidden names and empty lists.
    pub(crate) fn to_preferred(&self) -> Result<russh::Preferred> {
        fn check<'a>(kind: &str, list: &'a [String]) -> Result<&'a [String]> {
            if list.is_empty() {
                return Err(Error::InvalidConfig(format!("{kind} list is empty")));
            }
            if let Some(bad) = list.iter().find(|n| FORBIDDEN.contains(&n.as_str())) {
                return Err(Error::InvalidConfig(format!(
                    "{kind} algorithm `{bad}` is not allowed"
                )));
            }
            Ok(list)
        }
        fn unknown(kind: &str, name: &str) -> Error {
            Error::InvalidConfig(format!("unsupported {kind} algorithm `{name}`"))
        }

        let mut kex = Vec::new();
        for n in check("kex", &self.kex)? {
            if n == STRICT_KEX_CLIENT || n == EXT_INFO_CLIENT {
                continue;
            }
            kex.push(russh::kex::Name::try_from(n.as_str()).map_err(|()| unknown("kex", n))?);
        }
        kex.push(russh::kex::EXTENSION_SUPPORT_AS_CLIENT);
        kex.push(russh::kex::EXTENSION_OPENSSH_STRICT_KEX_AS_CLIENT);

        let mut key = Vec::new();
        for n in check("host key", &self.host_key)? {
            match Algorithm::new(n) {
                Ok(Algorithm::Other(_)) | Err(_) => return Err(unknown("host key", n)),
                Ok(a) => key.push(a),
            }
        }
        let cipher = check("cipher", &self.cipher)?
            .iter()
            .map(|n| russh::cipher::Name::try_from(n.as_str()).map_err(|()| unknown("cipher", n)))
            .collect::<Result<Vec<_>>>()?;
        let mac = check("mac", &self.mac)?
            .iter()
            .map(|n| russh::mac::Name::try_from(n.as_str()).map_err(|()| unknown("mac", n)))
            .collect::<Result<Vec<_>>>()?;

        Ok(russh::Preferred {
            kex: Cow::Owned(kex),
            key: Cow::Owned(key),
            host_key_certificates: Cow::Borrowed(&[]),
            cipher: Cow::Owned(cipher),
            mac: Cow::Owned(mac),
            compression: Cow::Borrowed(&[russh::compression::NONE]),
        })
    }
}

/// Terminal parameters for [`crate::Session::open_shell`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyRequest {
    /// `TERM` value, e.g. `xterm-256color`.
    pub term: String,
    /// Columns.
    pub cols: u32,
    /// Rows.
    pub rows: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_translates_and_forces_strict_kex() {
        let p = CryptoPolicy::default().to_preferred().unwrap();
        let names: Vec<&str> = p.kex.iter().map(|k| k.as_ref()).collect();
        assert_eq!(names.first(), Some(&"mlkem768x25519-sha256"));
        assert!(names.contains(&STRICT_KEX_CLIENT));
        assert!(names.contains(&EXT_INFO_CLIENT));
        assert!(!p.key.contains(&Algorithm::Rsa { hash: None }));
        assert_eq!(p.compression.len(), 1);
    }

    #[test]
    fn strict_kex_cannot_be_removed() {
        let policy = CryptoPolicy {
            kex: vec!["curve25519-sha256".into()],
            ..CryptoPolicy::default()
        };
        let p = policy.to_preferred().unwrap();
        assert!(p.kex.iter().any(|k| k.as_ref() == STRICT_KEX_CLIENT));
    }

    #[test]
    fn legacy_and_unknown_algorithms_are_rejected() {
        for (field, name) in [
            ("host_key", "ssh-rsa"),
            ("kex", "diffie-hellman-group1-sha1"),
            ("cipher", "aes256-cbc"),
            ("mac", "hmac-sha1"),
            ("kex", "sntrup761x25519-sha512@openssh.com"),
            ("host_key", "made-up"),
        ] {
            let mut p = CryptoPolicy::default();
            let list = match field {
                "host_key" => &mut p.host_key,
                "kex" => &mut p.kex,
                "cipher" => &mut p.cipher,
                _ => &mut p.mac,
            };
            list.insert(0, name.into());
            assert!(
                matches!(p.to_preferred(), Err(Error::InvalidConfig(_))),
                "{name} accepted"
            );
        }
        let empty = CryptoPolicy {
            cipher: vec![],
            ..CryptoPolicy::default()
        };
        assert!(empty.to_preferred().is_err());
    }

    #[test]
    fn option_ranges_are_validated() {
        let ok = ConnectOptions::new("h", "u");
        ok.validate().unwrap();
        assert!(ok.window_size >= MIN_WINDOW_SIZE);
        assert_eq!(ok.max_packet_size, MAX_PACKET_SIZE);

        let mut o = ok.clone();
        o.window_size = 1024;
        assert!(o.validate().is_err());
        let mut o = ok.clone();
        o.max_packet_size = MAX_PACKET_SIZE + 1;
        assert!(o.validate().is_err());
        let mut o = ok.clone();
        o.keepalive_max_missed = 0;
        assert!(o.validate().is_err());
        let mut o = ok;
        o.jump_hosts.push(JumpHop {
            host: String::new(),
            port: 22,
            user: "u".into(),
            auth: vec![],
        });
        assert!(o.validate().is_err());
    }

    #[test]
    fn debug_redacts_secrets() {
        let m = AuthMethod::Password(Some(Zeroizing::new("hunter2".into())));
        let k = AuthMethod::PublicKeyFile {
            path: "/k".into(),
            passphrase: Some(Zeroizing::new("pp-secret".into())),
        };
        let s = format!("{m:?} {k:?}");
        assert!(!s.contains("hunter2") && !s.contains("pp-secret"));
    }
}
