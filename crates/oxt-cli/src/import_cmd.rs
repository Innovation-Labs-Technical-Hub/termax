//! `oxt import …`: bring OpenSSH config and known_hosts into the vault.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use oxt_interop::{HostSpec, KnownHosts, ParseOptions, SshConfig};
use oxt_vault::KnownHost;
use serde::Serialize;

use crate::vault_cmd::{default_vault, open_store};
use crate::{Ctx, print_json};

#[derive(Subcommand)]
pub enum ImportCmd {
    /// Import hosts from an OpenSSH client config (default ~/.ssh/config).
    SshConfig {
        path: Option<PathBuf>,
        /// Show what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Import trusted host keys from a known_hosts file (default ~/.ssh/known_hosts).
    KnownHosts {
        path: Option<PathBuf>,
        /// Show what would be imported without writing anything.
        #[arg(long)]
        dry_run: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: ImportCmd) -> Result<ExitCode> {
    match cmd {
        ImportCmd::SshConfig { path, dry_run } => ssh_config(ctx, path, dry_run),
        ImportCmd::KnownHosts { path, dry_run } => known_hosts(ctx, path, dry_run),
    }
}

fn parse_options() -> Result<ParseOptions> {
    ParseOptions::from_env().context("cannot determine the home directory")
}

/// Outcome of one import, printed for people or as JSON.
#[derive(Serialize, Default)]
struct Report {
    dry_run: bool,
    imported: Vec<String>,
    duplicates: Vec<String>,
    skipped: Vec<String>,
    warnings: Vec<String>,
}

impl Report {
    fn print(&self, ctx: &Ctx, what: &str) -> Result<()> {
        if ctx.json {
            return print_json(self);
        }
        let verb = if self.dry_run {
            "Would import"
        } else {
            "Imported"
        };
        println!("{verb} {} {what}", self.imported.len());
        for i in &self.imported {
            println!("  + {i}");
        }
        if !self.duplicates.is_empty() {
            println!(
                "Already in the vault, left unchanged: {}",
                self.duplicates.len()
            );
            for d in &self.duplicates {
                println!("  = {d}");
            }
        }
        if !self.skipped.is_empty() {
            println!("Skipped: {}", self.skipped.len());
            for s in &self.skipped {
                println!("  - {s}");
            }
        }
        for w in &self.warnings {
            eprintln!("warning: {w}");
        }
        Ok(())
    }
}

fn ssh_config(ctx: &Ctx, path: Option<PathBuf>, dry_run: bool) -> Result<ExitCode> {
    let options = parse_options()?;
    let path = path.unwrap_or_else(|| options.ssh_dir.join("config"));
    let config = SshConfig::parse_file(&path, &options)
        .with_context(|| format!("reading {}", path.display()))?;

    let mut store = open_store(ctx)?;
    let vault = default_vault(&store)?;
    let parsed = config.import(vault.id);

    let existing: HashSet<String> = store
        .list_hosts(vault.id, &Default::default())?
        .into_iter()
        .map(|h| h.label.to_lowercase())
        .collect();

    let mut report = Report {
        dry_run,
        warnings: parsed.warnings.iter().map(ToString::to_string).collect(),
        skipped: parsed
            .skipped
            .iter()
            .map(|s| format!("{} ({})", s.pattern, s.reason))
            .collect(),
        ..Default::default()
    };
    for host in &parsed.hosts {
        if existing.contains(&host.label.to_lowercase()) {
            report.duplicates.push(host.label.clone());
            continue;
        }
        if !dry_run {
            store.create_host(host)?;
        }
        report
            .imported
            .push(format!("{} -> {}", host.label, host.address));
    }
    report.print(ctx, "hosts")?;
    Ok(ExitCode::SUCCESS)
}

fn known_hosts(ctx: &Ctx, path: Option<PathBuf>, dry_run: bool) -> Result<ExitCode> {
    let path = match path {
        Some(p) => p,
        None => parse_options()?.ssh_dir.join("known_hosts"),
    };
    let parsed =
        KnownHosts::parse_file(&path).with_context(|| format!("reading {}", path.display()))?;
    let source = format!("import:{}", path.display());

    let mut store = open_store(ctx)?;
    let mut report = Report {
        dry_run,
        warnings: parsed.warnings.iter().map(ToString::to_string).collect(),
        ..Default::default()
    };

    for entry in &parsed.entries {
        // The vault stores only keys trusted for one concrete host. CA and
        // revocation lines would change meaning if stored as plain trust, and
        // hashed names cannot be recovered.
        let at = format!("line {}", entry.line);
        if entry.marker.is_some() {
            report.skipped.push(format!(
                "{at}: @cert-authority/@revoked lines are not imported yet"
            ));
            continue;
        }
        let patterns = match &entry.hosts {
            HostSpec::Hashed { .. } => {
                report
                    .skipped
                    .push(format!("{at}: hashed host name cannot be recovered"));
                continue;
            }
            HostSpec::Patterns(p) => p,
        };
        let fingerprint = entry.fingerprint_sha256();
        for pattern in patterns {
            let Some(host_port) = concrete_host_port(pattern) else {
                report
                    .skipped
                    .push(format!("{at}: pattern `{pattern}` is not a single host"));
                continue;
            };
            let label = format!("{host_port} {}", entry.key_type);
            match store.get_known_host(&host_port, &entry.key_type)? {
                Some(existing) if existing.fp_sha256 == fingerprint => report.duplicates.push(label),
                Some(_) => report.skipped.push(format!(
                    "{label}: a different key is already trusted for this host; resolve it in the app"
                )),
                None => {
                    if !dry_run {
                        store.upsert_known_host(&KnownHost::new(
                            &host_port,
                            &entry.key_type,
                            &fingerprint,
                            &source,
                        ))?;
                    }
                    report.imported.push(format!("{label} {fingerprint}"));
                }
            }
        }
    }
    report.print(ctx, "host keys")?;
    Ok(ExitCode::SUCCESS)
}

/// `host` → `host:22`, `[host]:2222` → `host:2222`; wildcards and negations → `None`.
fn concrete_host_port(pattern: &str) -> Option<String> {
    if pattern.is_empty() || pattern.starts_with('!') || pattern.contains(['*', '?']) {
        return None;
    }
    if let Some(rest) = pattern.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        let port: u16 = port.parse().ok()?;
        return (!host.is_empty()).then(|| format!("{}:{port}", host.to_lowercase()));
    }
    Some(format!("{}:22", pattern.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::concrete_host_port;

    #[test]
    fn host_patterns_normalise_to_host_port() {
        assert_eq!(
            concrete_host_port("Example.com").as_deref(),
            Some("example.com:22")
        );
        assert_eq!(concrete_host_port("[db]:2222").as_deref(), Some("db:2222"));
        assert_eq!(
            concrete_host_port("10.0.0.1").as_deref(),
            Some("10.0.0.1:22")
        );
        assert_eq!(concrete_host_port("*.corp"), None);
        assert_eq!(concrete_host_port("!bad"), None);
        assert_eq!(concrete_host_port("[db]:notaport"), None);
    }
}
