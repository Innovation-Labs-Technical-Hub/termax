# xtask

## Purpose

`xtask` holds the Termax developer tasks that are better written in Rust than in the justfile. `cargo xtask perf` measures the terminal core (parser throughput, snapshot cost, echo latency under flood, memory per tab) and the SSH layer against a local sshd. It compares the results with the performance targets and fails on regressions against a baseline run, so CI can gate on it. `cargo xtask sshd` runs the test fixture's throwaway sshd in the foreground for manual use. The crate is `publish = false` and is never shipped.

## Public API entry points

This is a binary crate. The `cargo xtask` alias in `.cargo/config.toml` runs it in release mode.

- `cargo xtask perf [--backend ghostty|alacritty] [--check] [--baseline FILE] [--tolerance F] [--json FILE] [--mib N] [--tabs N] [--no-ssh]`. The default backend is `ghostty` on Linux and `alacritty` elsewhere. The default tolerance is 0.10 (10 %).
- `cargo xtask sshd`.

## Commands

- `cargo test -p xtask` (unit tests for the metric and regression logic).
- `cargo clippy -p xtask --all-targets -- -D warnings`.
- `just perf --no-ssh` and `just perf --backend alacritty` wrap `cargo xtask perf`.
- `just perf-baseline` saves `target/perf/baseline.json`. `just perf-compare` checks against it and fails on regressions over 10 %.
- `just compare-backends` (Linux) writes Ghostty and alacritty results side by side.
- `cargo xtask perf --no-ssh --mib 1 --tabs 2` is a quick smoke run. The defaults (64 MiB per workload, 20 tabs, SSH included) take much longer.
- `just sshd` (the same as `cargo xtask sshd`). Stop it with Ctrl-C.

## Layout

- `src/main.rs`: the clap commands and backend selection (Ghostty is compiled in only on Linux).
- `src/perf.rs`: terminal-core measurements, generic over a `TermCore` factory. `src/workloads.rs`: deterministic byte streams that stand in for real traffic.
- `src/ssh_perf.rs`: SSH measurements against `termax-sshd-fixture`, compared with the stock OpenSSH client where that makes sense.
- `src/metric.rs`: the `Metric` type, targets, gated and ungated metrics, and baseline comparison. `src/sshd.rs`: the foreground sshd command.

## Conventions

- A target and a regression are separate questions. A missed target is always reported but fails the run only with `--check`. A regression fails whenever `--baseline` is given. That way CI blocks slowdowns while a target is still out of reach.
- Mark a metric `ungated()` when it is too noisy or environment-dependent to gate on. Do not loosen the tolerance instead.
- Add new measurements generically over the backend factory, so both `TermCore` backends get the same numbers.
- Compare baselines only between runs on the same machine. A baseline recorded on another backend is noted and is not used as a regression baseline, so a deliberate backend switch does not read as a slowdown.

## Gotchas

- The `ghostty` backend value exists only on Linux. On other platforms `--backend ghostty` is a clap error.
- Without `--no-ssh`, the run needs `/usr/sbin/sshd` and `ssh-keygen` (see `termax-sshd-fixture`).
- Run perf in release mode (the alias already does) and on an otherwise idle machine. Debug numbers are meaningless.

## OpenSpec

- `platform-engineering`, module PL-01: FR-PL01-001 (Cargo workspace and xtask) and FR-PL01-009 (performance gates). Module CO-13: FR-CO13-004 (perf counters), as far as the harness goes.
- `product-foundation` `NFR-00-*` and the per-area performance NFRs (for example NFR-03 for the terminal) supply the targets that `perf` checks. Implementation is partial; check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
