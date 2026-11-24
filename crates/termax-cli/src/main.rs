//! `tmx` — Termax command-line tool. Shares the encrypted local vault with
//! the desktop app.

#![allow(clippy::print_stdout, reason = "a CLI: stdout is its output")]

mod hosts_cmd;
mod import_cmd;
mod policy_cmd;
mod vault_cmd;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tmx", version, about = "Termax command-line tool")]
struct Cli {
    /// Vault database path (default: the per-user Termax data directory).
    #[arg(long, global = true, env = "TMX_VAULT_PATH")]
    db: Option<PathBuf>,

    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create or inspect the local vault.
    #[command(subcommand)]
    Vault(vault_cmd::VaultCmd),
    /// Import hosts or host keys from OpenSSH files.
    #[command(subcommand)]
    Import(import_cmd::ImportCmd),
    /// List or export saved hosts.
    #[command(subcommand)]
    Hosts(hosts_cmd::HostsCmd),
    /// Evaluate commands against the agent approval policy.
    #[command(subcommand)]
    Policy(policy_cmd::PolicyCmd),
}

/// Options shared by every subcommand.
pub struct Ctx {
    pub db: Option<PathBuf>,
    pub json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx {
        db: cli.db,
        json: cli.json,
    };
    let result = match cli.command {
        Command::Vault(cmd) => vault_cmd::run(&ctx, cmd),
        Command::Import(cmd) => import_cmd::run(&ctx, cmd),
        Command::Hosts(cmd) => hosts_cmd::run(&ctx, cmd),
        Command::Policy(cmd) => policy_cmd::run(&ctx, cmd),
    };
    match result {
        Ok(code) => code,
        Err(err) => {
            eprintln!("tmx: {err:#}");
            ExitCode::from(1)
        }
    }
}

/// Print `value` as pretty JSON.
pub fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
