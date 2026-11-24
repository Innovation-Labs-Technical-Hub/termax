//! `cargo xtask perf` — measures the terminal core against the performance
//! targets in the architecture brief and, with `--check`, fails when a budget
//! is missed so CI can gate on it.

mod perf;
mod ssh_perf;
mod workloads;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the performance harness.
    Perf {
        /// Exit with status 1 if any budget is missed.
        #[arg(long)]
        check: bool,
        /// Also write the results as JSON to this file.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Bytes fed per throughput workload, in MiB.
        #[arg(long, default_value_t = 64)]
        mib: usize,
        /// Terminals created for the per-tab memory measurement.
        #[arg(long, default_value_t = 20)]
        tabs: usize,
        /// Skip the SSH measurements against a local sshd.
        #[arg(long)]
        no_ssh: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Perf {
            check,
            json,
            mib,
            tabs,
            no_ssh,
        } => perf::run(&perf::Options {
            check,
            json,
            bytes: mib << 20,
            tabs,
            ssh: !no_ssh,
        }),
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
