//! Error type for policy configuration and audit persistence.

use thiserror::Error;

/// Errors raised while loading rules or decoding an audit log.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// The rules file is not valid TOML or does not match the schema.
    #[error("invalid rules TOML: {0}")]
    Toml(#[from] toml::de::Error),
    /// An `args_regex` pattern failed to compile.
    #[error("invalid regex: {0}")]
    Regex(#[from] regex::Error),
    /// A rule is structurally invalid.
    #[error("invalid rule: {0}")]
    InvalidRule(String),
    /// An audit log line could not be decoded.
    #[error("invalid audit log line {line}: {source}")]
    AuditDecode {
        /// 1-based line number.
        line: usize,
        /// Underlying JSON error.
        source: serde_json::Error,
    },
}
