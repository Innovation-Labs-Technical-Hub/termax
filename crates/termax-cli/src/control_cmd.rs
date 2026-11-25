//! `tmx headless`, `tmx pair` and `tmx sessions …`: the local control
//! endpoint from the command line.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde_json::json;
use termax_control::protocol::{ListResult, OpenResult};
use termax_control::{
    ApprovalKind, ApprovalRequest, Approvals, ClientKind, ClientRegistry, ControlClient,
    ControlError, CtlCode, Endpoint, Headless, KeychainStore, Scope, ServerConfig, TokenStore,
    VaultOpener,
};
use termax_policy::{PolicyConfig, PolicyEngine};
use termax_session::{RuntimeConfig, SessionRuntime};

use crate::{Ctx, print_json};

/// Name this CLI pairs under.
const CLIENT_NAME: &str = "tmx";

#[derive(Subcommand)]
pub enum SessionsCmd {
    /// List live sessions of the running Termax runtime.
    List,
    /// Open a session to a saved host in the running runtime.
    Open {
        /// Saved host label.
        host: String,
        /// Terminal columns.
        #[arg(long, default_value_t = 80)]
        cols: u16,
        /// Terminal rows.
        #[arg(long, default_value_t = 24)]
        rows: u16,
    },
}

fn endpoint(ctx: &Ctx) -> Result<Endpoint> {
    match &ctx.endpoint {
        Some(dir) => Ok(Endpoint::in_dir(dir)),
        None => Endpoint::for_current_user().context("locating the control endpoint"),
    }
}

/// Keychain account for this CLI's token at `endpoint`.
fn account(endpoint: &Endpoint) -> String {
    format!("{CLIENT_NAME}@{}", endpoint.display())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")
}

/// Explain endpoint errors with what to do next.
fn explain(e: ControlError) -> anyhow::Error {
    match e.code() {
        Some(CtlCode::Unauthorized) => {
            anyhow::anyhow!("{e}\nhint: pair this CLI with `tmx pair` and approve it in Termax")
        }
        _ => match e {
            ControlError::NotRunning(_) => anyhow::anyhow!(
                "{e}\nhint: start the desktop app or run `tmx headless` in another terminal"
            ),
            other => other.into(),
        },
    }
}

/// Connect and authenticate with the stored token (if any).
async fn connect(ctx: &Ctx) -> Result<ControlClient> {
    let ep = endpoint(ctx)?;
    let client = ControlClient::connect(&ep).await.map_err(explain)?;
    let token = match KeychainStore.get(&account(&ep)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("tmx: cannot read the keychain ({e}); continuing unpaired");
            None
        }
    };
    client.hello(token).await.map_err(explain)?;
    Ok(client)
}

pub fn pair(ctx: &Ctx, operate: bool) -> Result<ExitCode> {
    let ep = endpoint(ctx)?;
    let scopes = if operate {
        vec![Scope::Observe, Scope::Operate]
    } else {
        vec![Scope::Observe]
    };
    let res = runtime()?.block_on(async {
        let client = ControlClient::connect(&ep).await.map_err(explain)?;
        eprintln!("Waiting for approval in Termax…");
        client
            .pair(CLIENT_NAME, ClientKind::Cli, scopes)
            .await
            .map_err(explain)
    })?;
    KeychainStore
        .set(&account(&ep), &res.token)
        .context("storing the client token in the OS keychain")?;
    if ctx.json {
        print_json(&json!({ "client_id": res.client_id, "scopes": res.scopes }))?;
    } else {
        println!("Paired as {} with scopes {:?}", res.client_id, res.scopes);
    }
    Ok(ExitCode::SUCCESS)
}

pub fn sessions(ctx: &Ctx, cmd: SessionsCmd) -> Result<ExitCode> {
    runtime()?.block_on(async {
        let client = connect(ctx).await?;
        match cmd {
            SessionsCmd::List => {
                let list: ListResult = client
                    .call("session.list", &json!({}))
                    .await
                    .map_err(explain)?;
                if ctx.json {
                    print_json(&list)?;
                } else if list.sessions.is_empty() {
                    println!("No sessions.");
                } else {
                    for s in &list.sessions {
                        let state = match &s.code {
                            Some(c) => format!("{} ({c})", s.state),
                            None => s.state.clone(),
                        };
                        println!("{}  {:<14} {}", s.id, state, s.address);
                    }
                }
            }
            SessionsCmd::Open { host, cols, rows } => {
                let opened: OpenResult = client
                    .call(
                        "session.open",
                        &json!({ "host": host, "cols": cols, "rows": rows }),
                    )
                    .await
                    .map_err(explain)?;
                if ctx.json {
                    print_json(&opened)?;
                } else {
                    println!("{}", opened.session);
                }
            }
        }
        Ok(ExitCode::SUCCESS)
    })
}

/// Ask on the terminal; anything but `y`/`yes` denies.
fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).is_ok()
        && matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn describe(req: &ApprovalRequest) -> String {
    match &req.kind {
        ApprovalKind::Pairing {
            name,
            kind,
            pid,
            binary_path,
            scopes,
        } => format!(
            "Pair client {name:?} ({kind:?}, pid {}, {}) with scopes {scopes:?}?",
            pid.map_or_else(|| "?".into(), |p| p.to_string()),
            binary_path.as_deref().unwrap_or("unknown binary"),
        ),
        ApprovalKind::Operate {
            client_name,
            method,
            session,
            summary,
            reason,
            ..
        } => format!(
            "Allow {client_name:?} to {summary} via {method} on session {session} ({reason})?"
        ),
    }
}

/// The headless approval hook: the user answers on this terminal; without
/// a terminal every request is denied.
async fn approve_on_terminal(approvals: Arc<Approvals>) {
    let interactive = std::io::stdin().is_terminal();
    let mut rx = approvals.subscribe();
    loop {
        let req = match rx.recv().await {
            Ok(r) => r,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return,
        };
        let question = describe(&req);
        let yes = if interactive {
            tokio::task::spawn_blocking(move || ask(&question))
                .await
                .unwrap_or(false)
        } else {
            eprintln!("tmx headless: no terminal to ask; denied: {question}");
            false
        };
        if yes {
            approvals.approve(req.id);
        } else {
            approvals.deny(req.id);
        }
    }
}

pub fn headless(ctx: &Ctx, idle_timeout: u64) -> Result<ExitCode> {
    if idle_timeout == 0 {
        bail!("--idle-timeout must be at least 1 second");
    }
    let ep = endpoint(ctx)?;
    let store = crate::vault_cmd::open_store(ctx)?;
    let vault = crate::vault_cmd::default_vault(&store)?;
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let opener = Arc::new(VaultOpener::new(
        Arc::new(Mutex::new(store)),
        vault.id,
        user,
    ));
    let registry = ClientRegistry::open(
        termax_control::paths::config_dir()?.join(termax_control::paths::REGISTRY_NAME),
    )?;
    let config = ServerConfig::new(
        Arc::new(registry),
        opener,
        Arc::new(PolicyEngine::new(PolicyConfig::default())),
    );
    let rt = SessionRuntime::new(RuntimeConfig::default())?;
    let headless = Headless::start(&ep, rt.service(), config)?;
    if ctx.json {
        print_json(&json!({ "endpoint": ep.display(), "idle_timeout_s": idle_timeout }))?;
    } else {
        println!("Termax headless runtime listening on {}", ep.display());
    }
    let approver = rt
        .handle()
        .spawn(approve_on_terminal(headless.control().approvals()));
    let idle = Duration::from_secs(idle_timeout);
    let stopped_by_user = rt.block_on(async {
        tokio::select! {
            () = headless.wait_until_idle(idle) => false,
            r = tokio::signal::ctrl_c() => r.is_ok(),
        }
    });
    approver.abort();
    rt.block_on(headless.shutdown());
    let report = rt.quit();
    if !ctx.json {
        eprintln!(
            "tmx headless: {} ({} sessions closed)",
            if stopped_by_user {
                "interrupted"
            } else {
                "idle, exiting"
            },
            report.sessions_closed
        );
    }
    Ok(ExitCode::SUCCESS)
}
