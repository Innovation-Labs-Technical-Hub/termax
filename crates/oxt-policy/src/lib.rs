//! Policy and approval engine for shell commands proposed by AI agents.
//!
//! Before an agent-proposed command runs on a remote host, [`PolicyEngine`]
//! decides [`Effect::Allow`], [`Effect::Ask`] or [`Effect::Deny`]:
//!
//! - the command is parsed with tree-sitter-bash into every simple command
//!   it would run ([`analyze`]), looking through pipelines, lists,
//!   subshells, substitutions and wrappers such as `sudo` or `bash -c`;
//! - built-in rules deny catastrophic commands and cannot be relaxed;
//! - user [`RuleSet`]s (loadable from TOML) apply per sub-command, and the
//!   most restrictive result wins (Deny > Ask > Allow, default Ask);
//! - production hosts never auto-allow, external clients are observe-only
//!   unless granted, and a [`KillSwitch`] and per-actor [`RateLimit`] apply;
//! - every decision is appended to a hash-chained [`AuditLog`].
//!
//! [`sanitize_output`] cleans command output before it reaches a model.

mod analysis;
mod audit;
mod builtin;
mod engine;
mod error;
mod rules;
mod sanitize;

pub use analysis::{Analysis, Issue, MAX_COMMAND_LEN, SimpleCommand, Word, analyze};
pub use audit::{AuditEntry, AuditError, AuditLog, GENESIS_HASH};
pub use engine::{
    Actor, Clock, EvalRequest, ExternalMode, KillSwitch, ManualClock, PolicyConfig, PolicyEngine,
    RateLimit, SystemClock,
};
pub use error::PolicyError;
pub use rules::{ArgMatcher, Effect, Rule, RuleSet, Verdict};
pub use sanitize::{REDACTED, sanitize_output};

#[cfg(test)]
mod tests;
