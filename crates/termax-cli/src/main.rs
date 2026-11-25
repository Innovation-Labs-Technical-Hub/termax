//! `tmx` — Termax command-line tool. Shares the encrypted local vault with
//! the desktop app, and reaches a running runtime (the app or `tmx
//! headless`) through the local control endpoint.

#![allow(clippy::print_stdout, reason = "a CLI: stdout is its output")]

mod control_cmd;
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

    /// Control endpoint directory (default: the per-user runtime directory).
    #[arg(long, global = true, env = "TMX_ENDPOINT")]
    endpoint: Option<PathBuf>,

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
    /// Run the session runtime and control endpoint without a UI.
    Headless {
        /// Exit after this many seconds with no sessions and no clients.
        #[arg(long, default_value_t = 300)]
        idle_timeout: u64,
    },
    /// Pair this CLI with the running Termax runtime (approve it there).
    Pair {
        /// Also request the operate scope (open sessions, send input).
        #[arg(long)]
        operate: bool,
    },
    /// Live sessions of the running Termax runtime.
    #[command(subcommand)]
    Sessions(control_cmd::SessionsCmd),
    /// Shorthand for `tmx sessions list`.
    Ls,
}

/// Options shared by every subcommand.
pub struct Ctx {
    pub db: Option<PathBuf>,
    pub json: bool,
    pub endpoint: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx {
        db: cli.db,
        json: cli.json,
        endpoint: cli.endpoint,
    };
    let result = match cli.command {
        Command::Vault(cmd) => vault_cmd::run(&ctx, cmd),
        Command::Import(cmd) => import_cmd::run(&ctx, cmd),
        Command::Hosts(cmd) => hosts_cmd::run(&ctx, cmd),
        Command::Policy(cmd) => policy_cmd::run(&ctx, cmd),
        Command::Headless { idle_timeout } => control_cmd::headless(&ctx, idle_timeout),
        Command::Pair { operate } => control_cmd::pair(&ctx, operate),
        Command::Sessions(cmd) => control_cmd::sessions(&ctx, cmd),
        Command::Ls => control_cmd::sessions(&ctx, control_cmd::SessionsCmd::List),
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
