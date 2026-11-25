//! Error type for the SSH facade.

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Result alias used across this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every failure the facade can report. Backend (russh) errors are flattened
/// into [`Error::Protocol`] so callers never depend on the backend's types.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The options or crypto policy are invalid (rejected before any I/O).
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// The TCP connection (or the jump-host tunnel) could not be opened.
    #[error("cannot connect to {target}: {reason}")]
    Connect {
        /// `host:port` being reached.
        target: String,
        /// Underlying cause.
        reason: String,
    },

    /// The connection, handshake or authentication did not finish in time.
    /// Time spent waiting on user prompts is not counted.
    #[error("{stage} timed out after {after:?}")]
    Timeout {
        /// What was in progress.
        stage: &'static str,
        /// The budget that elapsed.
        after: Duration,
    },

    /// The user rejected an unknown host key.
    #[error("host key for {host_port} rejected ({fingerprint})")]
    HostKeyRejected {
        /// Host whose key was rejected.
        host_port: String,
        /// Fingerprint of the rejected key.
        fingerprint: String,
    },

    /// The server presented a key that differs from the trusted one. The
    /// connection is always aborted; no prompt can override this.
    #[error(
        "HOST KEY CHANGED for {host_port}: trusted {old_key_type} {old_fingerprint}, \
         server sent {new_key_type} {new_fingerprint}; refusing to connect"
    )]
    HostKeyChanged {
        /// Host whose key changed.
        host_port: String,
        /// Type of the trusted key.
        old_key_type: String,
        /// Fingerprint of the trusted key.
        old_fingerprint: String,
        /// Type of the presented key.
        new_key_type: String,
        /// Fingerprint of the presented key.
        new_fingerprint: String,
    },

    /// The server presented a revoked key.
    #[error("host key for {host_port} is revoked ({fingerprint}); refusing to connect")]
    HostKeyRevoked {
        /// Host that presented the key.
        host_port: String,
        /// Fingerprint of the revoked key.
        fingerprint: String,
    },

    /// The trust store could not be read or written. Treated as a failure to
    /// verify (fail closed).
    #[error("host key store error: {0}")]
    HostKeyStore(String),

    /// Every configured authentication method failed.
    #[error("authentication failed for {user}: {}", AttemptList(.attempts))]
    AuthFailed {
        /// User that failed to authenticate.
        user: String,
        /// Methods the server said it accepts (from the last failure reply).
        server_methods: Vec<String>,
        /// What was tried, in order.
        attempts: Vec<AuthAttempt>,
    },

    /// The server reported success without verifying any credential.
    #[error(
        "server accepted `{method}` authentication without verifying a credential; \
         refusing (trivial authentication)"
    )]
    TrivialAuthRefused {
        /// Method the server claimed succeeded.
        method: &'static str,
    },

    /// The user cancelled a prompt that the connection cannot continue without.
    #[error("cancelled by user")]
    Cancelled,

    /// A private key file could not be read or decrypted.
    #[error("cannot load key {path}: {reason}")]
    KeyLoad {
        /// Key file.
        path: PathBuf,
        /// Why it could not be used.
        reason: String,
    },

    /// A local forward could not bind its listening socket.
    #[error("cannot bind local forward on {addr}: {reason}")]
    ForwardBind {
        /// Requested bind address.
        addr: String,
        /// Underlying cause (e.g. address in use).
        reason: String,
    },

    /// The server refused a request on a channel (pty, shell, exec) or the
    /// channel closed during setup.
    #[error("channel open failed: {0}")]
    ChannelOpen(String),

    /// The server answered a channel open with `SSH_MSG_CHANNEL_OPEN_FAILURE`
    /// (RFC 4254 §5.1). OpenSSH (10.2) reports `MaxSessions` as
    /// [`ChannelOpenFailure::ConnectFailed`]; other servers use
    /// `AdministrativelyProhibited` or `ResourceShortage`.
    #[error("server refused to open a channel: {0}")]
    ChannelRefused(ChannelOpenFailure),

    /// The operation is not supported by the SSH backend.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),

    /// The connection is closed.
    #[error("connection closed: {0}")]
    Closed(crate::session::CloseReason),

    /// The server stopped answering keepalives.
    #[error("server not responding to keepalives")]
    KeepaliveTimeout,

    /// A protocol or transport error from the SSH backend.
    #[error("ssh protocol error: {0}")]
    Protocol(String),

    /// Local I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl From<russh::Error> for Error {
    fn from(e: russh::Error) -> Self {
        match e {
            russh::Error::KeepaliveTimeout => Error::KeepaliveTimeout,
            russh::Error::ConnectionTimeout | russh::Error::InactivityTimeout => Error::Timeout {
                stage: "ssh transport",
                after: Duration::ZERO,
            },
            russh::Error::IO(io) => Error::Io(io),
            russh::Error::ChannelOpenFailure(f) => Error::ChannelRefused(f.into()),
            other => Error::Protocol(other.to_string()),
        }
    }
}

impl From<russh::SendError> for Error {
    fn from(_: russh::SendError) -> Self {
        Error::Protocol("session task is gone".into())
    }
}

/// Reason code of a refused channel open (RFC 4254 §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChannelOpenFailure {
    /// `SSH_OPEN_ADMINISTRATIVELY_PROHIBITED` (1).
    AdministrativelyProhibited,
    /// `SSH_OPEN_CONNECT_FAILED` (2); also what OpenSSH sends at `MaxSessions`.
    ConnectFailed,
    /// `SSH_OPEN_UNKNOWN_CHANNEL_TYPE` (3).
    UnknownChannelType,
    /// `SSH_OPEN_RESOURCE_SHORTAGE` (4).
    ResourceShortage,
    /// Any other code, with the server's description.
    Other {
        /// Reason code on the wire.
        code: u32,
        /// Server-provided description.
        description: String,
    },
}

impl fmt::Display for ChannelOpenFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelOpenFailure::AdministrativelyProhibited => {
                f.write_str("administratively prohibited")
            }
            ChannelOpenFailure::ConnectFailed => f.write_str("connect failed"),
            ChannelOpenFailure::UnknownChannelType => f.write_str("unknown channel type"),
            ChannelOpenFailure::ResourceShortage => f.write_str("resource shortage"),
            ChannelOpenFailure::Other { code, description } => {
                write!(f, "code {code}: {description}")
            }
        }
    }
}

impl From<russh::ChannelOpenFailure> for ChannelOpenFailure {
    fn from(f: russh::ChannelOpenFailure) -> Self {
        match f {
            russh::ChannelOpenFailure::AdministrativelyProhibited => {
                ChannelOpenFailure::AdministrativelyProhibited
            }
            russh::ChannelOpenFailure::ConnectFailed => ChannelOpenFailure::ConnectFailed,
            russh::ChannelOpenFailure::UnknownChannelType => ChannelOpenFailure::UnknownChannelType,
            russh::ChannelOpenFailure::ResourceShortage => ChannelOpenFailure::ResourceShortage,
            russh::ChannelOpenFailure::Other { code, reason } => ChannelOpenFailure::Other {
                code,
                description: reason,
            },
        }
    }
}

/// One authentication attempt and why it did not succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAttempt {
    /// SSH method name (`publickey`, `password`, `keyboard-interactive`).
    pub method: &'static str,
    /// Which credential (key path, `agent:<comment>`, ...). Never a secret.
    pub credential: String,
    /// Why it failed.
    pub outcome: AttemptOutcome,
}

/// Outcome of a failed [`AuthAttempt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The server rejected the credential.
    Rejected,
    /// The server accepted it but requires a further method.
    PartialSuccess,
    /// The server does not offer this method.
    NotOffered,
    /// The credential could not be used locally (unreadable key, empty agent, ...).
    Unavailable(String),
    /// The user skipped this credential at a prompt.
    Skipped,
}

impl fmt::Display for AuthAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.method)?;
        if !self.credential.is_empty() {
            write!(f, " ({})", self.credential)?;
        }
        match &self.outcome {
            AttemptOutcome::Rejected => write!(f, ": rejected"),
            AttemptOutcome::PartialSuccess => write!(f, ": partial success"),
            AttemptOutcome::NotOffered => write!(f, ": not offered by server"),
            AttemptOutcome::Unavailable(why) => write!(f, ": unavailable: {why}"),
            AttemptOutcome::Skipped => write!(f, ": skipped"),
        }
    }
}

struct AttemptList<'a>(&'a [AuthAttempt]);

impl fmt::Display for AttemptList<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return write!(f, "no methods tried");
        }
        write!(f, "tried ")?;
        for (i, a) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{a}")?;
        }
        Ok(())
    }
}

pub(crate) fn bind_error(addr: SocketAddr, e: &std::io::Error) -> Error {
    Error::ForwardBind {
        addr: addr.to_string(),
        reason: e.to_string(),
    }
}
