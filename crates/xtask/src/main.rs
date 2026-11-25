//! `cargo xtask perf` — measures the terminal core and SSH layer against the
//! performance targets in the architecture brief, and fails on regressions
//! against a baseline run so CI can gate on it. `cargo xtask spec-coverage`
//! reports which OpenSpec scenarios are linked to tests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "developer tool: prints reports, and a panic is an acceptable failure mode"
)]

mod metric;
mod perf;
mod session_perf;
mod spec_coverage;
mod ssh_perf;
mod sshd;
mod workloads;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use termax_term::{AlacrittyTerm, TermConfig, TermCore};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Backend {
    /// libghostty-vt; the default on Linux, where its Zig build works today.
    #[cfg(target_os = "linux")]
    Ghostty,
    /// alacritty_terminal; the default elsewhere and kept for comparison.
    Alacritty,
}

/// The default terminal backend for this platform.
#[cfg(target_os = "linux")]
const DEFAULT_BACKEND: Backend = Backend::Ghostty;
#[cfg(not(target_os = "linux"))]
const DEFAULT_BACKEND: Backend = Backend::Alacritty;

impl Backend {
    fn name(self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Backend::Ghostty => "libghostty-vt",
            Backend::Alacritty => "alacritty_terminal",
        }
    }

    fn factory(self) -> perf::Factory {
        match self {
            Backend::Alacritty => {
                |c: TermConfig| Box::new(AlacrittyTerm::new(c)) as Box<dyn TermCore>
            }
            #[cfg(target_os = "linux")]
            Backend::Ghostty => |c: TermConfig| {
                Box::new(termax_term_ghostty::GhosttyTerm::new(c)) as Box<dyn TermCore>
            },
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Run the performance harness.
    Perf {
        /// Terminal core to measure.
        #[arg(long, value_enum, default_value_t = DEFAULT_BACKEND)]
        backend: Backend,
        /// Also fail when a target from the architecture brief is missed.
        #[arg(long)]
        check: bool,
        /// Results of an earlier run on the same machine; gated metrics that
        /// got worse by more than the tolerance fail the run.
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Allowed relative regression versus the baseline.
        #[arg(long, default_value_t = perf::Options::default_tolerance())]
        tolerance: f64,
        /// Write the results as JSON to this file (usable as a baseline).
        #[arg(long)]
        json: Option<PathBuf>,
        /// Bytes fed per throughput workload, in MiB.
        #[arg(long, default_value_t = 64)]
        mib: usize,
        /// Terminals created for each per-tab memory measurement.
        #[arg(long, default_value_t = 20)]
        tabs: usize,
        /// Skip the SSH measurements against a local sshd.
        #[arg(long)]
        no_ssh: bool,
    },
    /// Start a throwaway OpenSSH sshd for manual testing (Ctrl-C stops it).
    Sshd,
    /// Report which OpenSpec scenarios are linked to tests (`/// Verifies:`
    /// tags); fails on tags that point nowhere.
    SpecCoverage {
        /// Count only requirements of this phase.
        #[arg(long, value_parser = ["P1", "P2", "P3"])]
        phase: Option<String>,
        /// Write the report as JSON to this file.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Run the tagged tests; a scenario counts as covered only when a
        /// linked test passes.
        #[arg(long)]
        run: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Perf {
            backend,
            check,
            baseline,
            tolerance,
            json,
            mib,
            tabs,
            no_ssh,
        } => perf::run(&perf::Options {
            backend_name: backend.name(),
            factory: backend.factory(),
            check,
            json,
            baseline,
            tolerance,
            bytes: mib << 20,
            tabs,
            ssh: !no_ssh,
        }),
        Command::Sshd => sshd::run().map(|()| true),
        Command::SpecCoverage { phase, json, run } => {
            spec_coverage::run(&spec_coverage::Options { phase, json, run })
        }
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(err) => {
            eprintln!("xtask: {err:#}");
            ExitCode::from(2)
        }
    }
}
