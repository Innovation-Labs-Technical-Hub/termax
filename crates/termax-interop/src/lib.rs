//! Import/export between Termax and OpenSSH client files.
//!
//! * [`ssh_config`] parses `~/.ssh/config` with ssh_config(5) semantics
//!   (`Include`, `Host`, basic `Match`, first-value-wins), resolves effective
//!   options for an alias like `ssh -G`, imports concrete host aliases as
//!   [`termax_core::Host`] records and exports hosts back to ssh_config text.
//! * [`known_hosts`] parses OpenSSH `known_hosts` files, including hashed
//!   entries and `@cert-authority` / `@revoked` markers, and computes
//!   OpenSSH-style SHA256 key fingerprints.
//!
//! Nothing in this crate ever executes commands found in configuration
//! (`ProxyCommand`, `Match exec`, `KnownHostsCommand`, ...); such values are
//! kept as plain strings.

mod error;
pub mod known_hosts;
mod pattern;
pub mod ssh_config;
mod warning;

pub use error::InteropError;
pub use known_hosts::{HostSpec, KnownHostEntry, KnownHosts, Marker, fingerprint_sha256};
pub use ssh_config::{
    ImportReport, MULTI_VALUE_SEPARATOR, ParseOptions, ResolvedConfig, SkippedPattern, SshConfig,
    decode_setting, encode_setting, to_ssh_config,
};
pub use warning::Warning;
