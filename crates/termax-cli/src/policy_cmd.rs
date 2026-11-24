//! `tmx policy check`: dry-run the agent approval policy on a command.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use termax_core::Env;
use termax_policy::{
    Actor, Effect, EvalRequest, ExternalMode, PolicyConfig, PolicyEngine, RuleSet,
};

use crate::{Ctx, print_json};

#[derive(Subcommand)]
pub enum PolicyCmd {
    /// Show whether a command would be allowed, need approval, or be denied.
    ///
    /// Exit status: 0 allow, 2 ask, 3 deny.
    Check {
        /// The shell command line to evaluate.
        command: String,
        /// Environment of the target host: none | dev | staging | prod.
        #[arg(long, default_value = "none")]
        env: String,
        /// Host tag (repeatable).
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// TOML file with user rules.
        #[arg(long)]
        rules: Option<PathBuf>,
        /// Evaluate as this external MCP client instead of the in-app assistant.
        #[arg(long)]
        external: Option<String>,
        /// With --external, evaluate as if the client had been granted operate mode.
        #[arg(long, requires = "external")]
        operate: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: PolicyCmd) -> Result<ExitCode> {
    let PolicyCmd::Check {
        command,
        env,
        tags,
        rules,
        external,
        operate,
    } = cmd;

    let env: Env = env.parse().map_err(|m: String| anyhow!(m))?;
    let rules = match rules {
        Some(path) => {
            let src = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            RuleSet::from_toml(&src).with_context(|| format!("parsing {}", path.display()))?
        }
        None => RuleSet::default(),
    };
    let engine = PolicyEngine::new(PolicyConfig {
        rules,
        rate_limit: None,
    });
    let actor = match external {
        Some(client_id) => {
            if operate {
                engine.set_external_mode(&client_id, ExternalMode::Operate);
            }
            Actor::External { client_id }
        }
        None => Actor::InApp,
    };
    let verdict = engine.evaluate(&EvalRequest {
        actor: &actor,
        command: &command,
        env,
        tags: &tags,
    });

    if ctx.json {
        print_json(&verdict)?;
    } else {
        let label = match verdict.effect {
            Effect::Allow => "allow",
            Effect::Ask => "ask",
            Effect::Deny => "deny",
        };
        println!("{label}");
        for reason in &verdict.reasons {
            println!("  - {reason}");
        }
    }
    Ok(ExitCode::from(match verdict.effect {
        Effect::Allow => 0,
        Effect::Ask => 2,
        Effect::Deny => 3,
    }))
}
