//! Stable `TMX-CTL-NNN` codes and the crate's error type.

use std::fmt;
use std::io;

/// A control-endpoint refusal or failure, carried in a JSON-RPC error's
/// `data.code`. Codes are never renumbered or reused; each has a section in
/// `docs/error-codes.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CtlCode {
    /// `TMX-CTL-400` malformed request or unsupported `api_version`.
    BadRequest,
    /// `TMX-CTL-401` no, unknown or revoked token, or pairing denied.
    Unauthorized,
    /// `TMX-CTL-403` the client's scope or the policy does not allow the call.
    Forbidden,
    /// `TMX-CTL-404` no such session.
    NotFound,
    /// `TMX-CTL-429` too many calls.
    RateLimited,
    /// `TMX-CTL-500` the operation failed.
    Failed,
}

impl CtlCode {
    /// Every code, in documentation order.
    pub const ALL: [CtlCode; 6] = [
        CtlCode::BadRequest,
        CtlCode::Unauthorized,
        CtlCode::Forbidden,
        CtlCode::NotFound,
        CtlCode::RateLimited,
        CtlCode::Failed,
    ];

    /// The code as shown to users.
    pub fn as_str(self) -> &'static str {
        match self {
            CtlCode::BadRequest => "TMX-CTL-400",
            CtlCode::Unauthorized => "TMX-CTL-401",
            CtlCode::Forbidden => "TMX-CTL-403",
            CtlCode::NotFound => "TMX-CTL-404",
            CtlCode::RateLimited => "TMX-CTL-429",
            CtlCode::Failed => "TMX-CTL-500",
        }
    }

    /// Parse a code string.
    pub fn parse(s: &str) -> Option<CtlCode> {
        CtlCode::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// English title (also the `docs/error-codes.md` heading text).
    pub fn title(self) -> &'static str {
        match self {
            CtlCode::BadRequest => "Bad control request",
            CtlCode::Unauthorized => "Client not paired",
            CtlCode::Forbidden => "Not allowed for this client",
            CtlCode::NotFound => "No such session",
            CtlCode::RateLimited => "Too many control requests",
            CtlCode::Failed => "Control operation failed",
        }
    }
}

impl fmt::Display for CtlCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Errors from setting up or using the endpoint.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ControlError {
    /// An I/O error.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The endpoint directory or socket is not private to this user, so the
    /// endpoint refuses to use it.
    #[error("insecure control endpoint: {0}")]
    Insecure(String),
    /// Another runtime of this user already holds the endpoint.
    #[error("another Termax runtime is already running for this user ({0})")]
    AlreadyRunning(String),
    /// No endpoint is listening.
    #[error("no Termax runtime is running (no control endpoint at {0})")]
    NotRunning(String),
    /// The socket path does not fit in a Unix socket address.
    #[error("control socket path is too long ({len} bytes, limit {limit}): {path}")]
    PathTooLong {
        /// The path.
        path: String,
        /// Its length in bytes.
        len: usize,
        /// The platform limit.
        limit: usize,
    },
    /// The per-user directory could not be determined.
    #[error("cannot determine the per-user runtime directory")]
    NoRuntimeDir,
    /// A malformed or oversized frame or message.
    #[error("control protocol error: {0}")]
    Protocol(String),
    /// The server answered with an error.
    #[error("{message}{}", code.map(|c| format!(" ({c})")).unwrap_or_default())]
    Rpc {
        /// JSON-RPC error code.
        rpc: i64,
        /// Stable code, when the server sent one.
        code: Option<CtlCode>,
        /// Server message.
        message: String,
        /// Extra data (for example `approval_id`).
        data: Option<serde_json::Value>,
    },
    /// The connection closed.
    #[error("control connection closed")]
    Closed,
    /// The client registry could not be read or written.
    #[error("client registry {path}: {reason}")]
    Registry {
        /// Registry file.
        path: String,
        /// What failed.
        reason: String,
    },
    /// The OS keychain failed.
    #[error("keychain: {0}")]
    Keychain(String),
    /// The OS random number generator failed.
    #[error("random number generator: {0}")]
    Random(String),
}

impl ControlError {
    /// The stable code of a server error, if any.
    pub fn code(&self) -> Option<CtlCode> {
        match self {
            ControlError::Rpc { code, .. } => *code,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_every_code_is_documented() {
        let docs = include_str!("../../../docs/error-codes.md");
        for c in CtlCode::ALL {
            assert_eq!(CtlCode::parse(c.as_str()), Some(c));
            let heading = format!("## {}", c.as_str());
            let section = docs
                .split("\n## ")
                .find(|s| format!("## {s}").starts_with(&heading))
                .unwrap_or_else(|| panic!("docs/error-codes.md has no section for {c}"));
            assert!(
                section.contains(c.title()),
                "section for {c} names its title"
            );
        }
        assert_eq!(CtlCode::parse("TMX-CTL-999"), None);
    }
}
