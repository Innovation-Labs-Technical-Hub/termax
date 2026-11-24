//! Mapping between ssh_config and [`termax_core::Host`].
//!
//! # Settings encoding
//!
//! Imported options are stored in [`Host::settings`] under the lower-case
//! ssh keyword (`user`, `identityfile`, `proxyjump`, `localforward`, ...).
//! `HostName` and `Port` map to [`Host::address`] and [`Host::port`]
//! instead. Each value is encoded as follows (see [`encode_setting`] and
//! [`decode_setting`]):
//!
//! * one directive's arguments are joined by single spaces, each argument
//!   quoted ssh_config-style when it contains whitespace, quotes, `\` or a
//!   leading `#` (e.g. `localforward` = `8080 localhost:80`);
//! * command keywords (`proxycommand`, `localcommand`, `remotecommand`,
//!   `knownhostscommand`) keep the raw command string unquoted;
//! * several directives of a multi-valued keyword (`identityfile`,
//!   `certificatefile`, `localforward`, `remoteforward`, `dynamicforward`,
//!   `sendenv`) are joined with [`MULTI_VALUE_SEPARATOR`] (`\n`), which can
//!   never occur inside an ssh_config value.
//!
//! Values are stored unexpanded (`~`, `%h`, ... are kept) so that they stay
//! portable and export back verbatim.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::PathBuf;

use termax_core::{Host, Protocol, VaultId};

use super::lexer::{join_args, quote_arg, split_args};
use super::resolve::{expand_hostname, first};
use super::{Condition, SshConfig, Warning, keywords};
use crate::pattern::is_wildcard;

/// Separator between the values of a multi-valued keyword in a setting.
pub const MULTI_VALUE_SEPARATOR: char = '\n';

/// Result of [`SshConfig::import`].
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// One host per concrete `Host` alias, in file order.
    pub hosts: Vec<Host>,
    /// Parse warnings plus problems found while mapping hosts.
    pub warnings: Vec<Warning>,
    /// `Host` patterns that could not become a host (wildcards, negations).
    pub skipped: Vec<SkippedPattern>,
}

/// A `Host` pattern that was not imported as a host of its own. Its
/// options still apply to matching aliases through resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPattern {
    pub pattern: String,
    pub reason: String,
    pub file: Option<PathBuf>,
    pub line: usize,
}

/// Encode the occurrences of `keyword` into a single setting value.
pub fn encode_setting(keyword: &str, occurrences: &[Vec<String>]) -> String {
    let raw = keywords::is_raw_line(&keyword.to_ascii_lowercase());
    occurrences
        .iter()
        .map(|args| if raw { args.join(" ") } else { join_args(args) })
        .collect::<Vec<_>>()
        .join(&MULTI_VALUE_SEPARATOR.to_string())
}

/// Decode a setting value back into one argument list per occurrence.
///
/// Fails if a non-command value has unbalanced quotes.
pub fn decode_setting(keyword: &str, value: &str) -> Result<Vec<Vec<String>>, String> {
    let raw = keywords::is_raw_line(&keyword.to_ascii_lowercase());
    value
        .split(MULTI_VALUE_SEPARATOR)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            if raw {
                Ok(vec![line.trim().to_owned()])
            } else {
                split_args(line)
            }
        })
        .collect()
}

impl SshConfig {
    /// Import one [`Host`] per concrete (non-wildcard, non-negated) `Host`
    /// alias. Options from every block that applies to the alias (including
    /// wildcard `Host` and `Match` blocks) are folded in by resolution.
    pub fn import(&self, vault_id: VaultId) -> ImportReport {
        let mut report = ImportReport {
            hosts: Vec::new(),
            warnings: self.warnings.clone(),
            skipped: Vec::new(),
        };
        let mut seen = HashSet::new();
        for cond in &self.conditions {
            let Condition::Host {
                patterns,
                file,
                line,
            } = cond
            else {
                continue;
            };
            for pattern in patterns {
                if is_wildcard(pattern) {
                    let reason = if pattern.starts_with('!') {
                        "negated pattern"
                    } else {
                        "wildcard pattern"
                    };
                    report.skipped.push(SkippedPattern {
                        pattern: pattern.clone(),
                        reason: reason.to_owned(),
                        file: file.clone(),
                        line: *line,
                    });
                    continue;
                }
                if !seen.insert(pattern.to_lowercase()) {
                    continue;
                }
                let host = self.import_alias(
                    pattern,
                    vault_id,
                    file.as_deref(),
                    *line,
                    &mut report.warnings,
                );
                report.hosts.push(host);
            }
        }
        report
    }

    fn import_alias(
        &self,
        alias: &str,
        vault_id: VaultId,
        file: Option<&std::path::Path>,
        line: usize,
        warnings: &mut Vec<Warning>,
    ) -> Host {
        let options = self.collect(alias);
        let mut expand_warnings = Vec::new();
        let address = expand_hostname(&options, alias, &mut expand_warnings);
        warnings.extend(
            expand_warnings
                .into_iter()
                .map(|w| Warning::new(file, Some(line), format!("{alias}: {}", w.message))),
        );
        let mut host = Host::new(vault_id, alias, address);
        host.protocol = Protocol::Ssh;
        if let Some(port) = first(&options, "port") {
            match port.parse::<u16>() {
                Ok(p) if p != 0 => host.port = Some(p),
                _ => warnings.push(Warning::new(
                    file,
                    Some(line),
                    format!("{alias}: invalid Port `{port}` ignored"),
                )),
            }
        }
        for (key, occurrences) in &options {
            if key == "hostname" || key == "port" {
                continue;
            }
            host.settings
                .set(key.clone(), encode_setting(key, occurrences));
        }
        host
    }
}

/// Render hosts as ssh_config text that [`SshConfig::import`] maps back to
/// the same label, address, port and settings.
///
/// Only [`Protocol::Ssh`] hosts are exported. Settings whose key is not an
/// ssh_config keyword (or is `hostname`, `port`, `host`, `match`,
/// `include`) are omitted. Labels are written as the `Host` alias with
/// `*`, `?` and `!` replaced by `_`; an empty label falls back to the
/// address. A non-command value line with unbalanced quotes is written as
/// one quoted argument. Hosts sharing a label shadow each other in ssh, as
/// first-value-wins applies.
pub fn to_ssh_config(hosts: &[Host]) -> String {
    let mut out = String::new();
    for host in hosts.iter().filter(|h| h.protocol == Protocol::Ssh) {
        let label = if host.label.trim().is_empty() {
            host.address.clone()
        } else {
            host.label.replace(['*', '?', '!'], "_")
        };
        if !out.is_empty() {
            out.push('\n');
        }
        // Writing to a String cannot fail.
        let _ = writeln!(out, "Host {}", quote_arg(&label));
        if host.address != label {
            let _ = writeln!(
                out,
                "    HostName {}",
                quote_arg(&host.address.replace('%', "%%"))
            );
        }
        if let Some(port) = host.port {
            let _ = writeln!(out, "    Port {port}");
        }
        for (key, value) in &host.settings.0 {
            let lower = key.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "hostname" | "port" | "host" | "match" | "include"
            ) {
                continue;
            }
            let Some(name) = keywords::canonical(&lower) else {
                continue;
            };
            let raw = keywords::is_raw_line(&lower);
            for line in value.split(MULTI_VALUE_SEPARATOR) {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let rendered = if raw || split_args(line).is_ok() {
                    line.to_owned()
                } else {
                    quote_arg(line)
                };
                let _ = writeln!(out, "    {name} {rendered}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setting_encoding_round_trips() {
        let occ = vec![
            vec!["~/.ssh/id one".to_string()],
            vec!["~/.ssh/id_two".to_string()],
        ];
        let enc = encode_setting("IdentityFile", &occ);
        assert_eq!(enc, "\"~/.ssh/id one\"\n~/.ssh/id_two");
        assert_eq!(decode_setting("identityfile", &enc).unwrap(), occ);

        let cmd = vec![vec!["ssh -W \"%h:%p\" bastion".to_string()]];
        let enc = encode_setting("proxycommand", &cmd);
        assert_eq!(enc, "ssh -W \"%h:%p\" bastion");
        assert_eq!(decode_setting("ProxyCommand", &enc).unwrap(), cmd);
    }

    #[test]
    fn export_sanitises_labels_and_skips_foreign_keys() {
        let vault = VaultId::new();
        let mut h = Host::new(vault, "web *", "10.0.0.1");
        h.settings.set("font_size", "13");
        h.settings.set("user", "deploy");
        let mut telnet = Host::new(vault, "sw", "sw.lan");
        telnet.protocol = Protocol::Telnet;
        let text = to_ssh_config(&[h, telnet]);
        assert_eq!(
            text,
            "Host \"web _\"\n    HostName 10.0.0.1\n    User deploy\n"
        );
    }
}
