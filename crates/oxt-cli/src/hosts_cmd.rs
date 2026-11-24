//! `oxt hosts …`: list and export saved hosts.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use oxt_core::{Env, Host};
use oxt_vault::HostFilter;

use crate::vault_cmd::{default_vault, open_store};
use crate::{Ctx, print_json};

#[derive(Subcommand)]
pub enum HostsCmd {
    /// List hosts in the vault.
    List {
        /// Case-insensitive text matched against label and address.
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        /// dev | staging | prod
        #[arg(long)]
        env: Option<String>,
    },
    /// Print SSH hosts as an OpenSSH client config.
    Export,
}

pub fn run(ctx: &Ctx, cmd: HostsCmd) -> Result<ExitCode> {
    let store = open_store(ctx)?;
    let vault = default_vault(&store)?;
    match cmd {
        HostsCmd::List { search, tag, env } => {
            let filter = HostFilter {
                text: search,
                tag,
                env: env
                    .map(|e| e.parse::<Env>().map_err(|m| anyhow!(m)))
                    .transpose()?,
                ..Default::default()
            };
            let hosts = store.list_hosts(vault.id, &filter)?;
            if ctx.json {
                print_json(&hosts)?;
            } else if hosts.is_empty() {
                println!("No hosts.");
            } else {
                for host in &hosts {
                    println!("{}", list_line(host));
                }
            }
        }
        HostsCmd::Export => {
            let hosts = store.list_hosts(vault.id, &HostFilter::default())?;
            print!("{}", oxt_interop::to_ssh_config(&hosts));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn list_line(host: &Host) -> String {
    let port = host
        .effective_port()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    let env = match host.env {
        Env::None => "",
        Env::Dev => "  [dev]",
        Env::Staging => "  [staging]",
        Env::Prod => "  [prod]",
    };
    format!("{:<24} {}{port}{env}", host.label, host.address)
}
