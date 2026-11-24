//! `cargo xtask perf` — measures the terminal core and SSH layer against the
//! performance targets in the architecture brief, and fails on regressions
//! against a baseline run so CI can gate on it.

mod metric;
mod perf;
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
    Alacritty,
    /// Requires building xtask with `--features ghostty`.
    #[cfg(feature = "ghostty")]
    Ghostty,
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Alacritty => "alacritty_terminal",
            #[cfg(feature = "ghostty")]
            Backend::Ghostty => "libghostty-vt",
        }
    }

    fn factory(self) -> perf::Factory {
        match self {
            Backend::Alacritty => {
                |c: TermConfig| Box::new(AlacrittyTerm::new(c)) as Box<dyn TermCore>
            }
            #[cfg(feature = "ghostty")]
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
        #[arg(long, value_enum, default_value_t = Backend::Alacritty)]
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
