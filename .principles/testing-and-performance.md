# Testing and performance principles

Scope: tests in every crate (`src/**` unit tests, `crates/*/tests/`), the perf harness (`crates/xtask`), the sshd fixture (`crates/termax-sshd-fixture`) and the CI jobs that run them.

## Principles

1. **Test against the real thing when it is cheap.** SSH is tested against a real, unprivileged OpenSSH sshd; terminal backends are held to the same expectations by running shared tests against both. Mocks hide protocol bugs. (`plans/reports/termax-ssh-261124-interop-matrix.md`.)
2. **Acceptance scenarios become tests.** Each OpenSpec `<id> acceptance N` scenario maps to an automated test or a perf metric, named in the change's `tasks.md`.
3. **Measure what users feel, gate what is stable.** Targets come from the brief (NFR-00-*); CI fails on regressions of stable metrics against the PR's base commit, not on aspirational targets.
4. **Never weaken a test to make it pass.** Fix the code, or document the behaviour difference in the test with the reason.

## Rules

1. MUST add or update tests in the same change as the behaviour; new public APIs need at least one test.
2. MUST keep tests deterministic: inject clocks, generate keys and fixtures at test time, use `tempfile` for paths, and never depend on the developer's `~/.ssh`, network or keychain (`ParseOptions::new(home, user)`, `HOME` overridden in CLI tests).
3. MUST let SSH interop tests skip with a clear message when `/usr/sbin/sshd` or `ssh-keygen` is missing, and MUST kill every spawned process on drop (`Sshd`, `KillOnDrop`). Leave no sshd or agent running after a test run.
4. MUST bound every async test with a timeout (`within(…)` in the SSH tests); a hung test is a failing test.
5. MUST run backend-agnostic `TermCore` expectations against every backend that builds on the platform (`just term-test`).
6. MUST record perf results with their backend (`cargo xtask perf --json`) and only compare like with like; the regression gate is skipped across a backend switch.
7. MUST gate only stable metrics: parser throughput, snapshot cost, flood echo latency and the core memory estimate are gated; RSS, SSH loopback numbers and connect time are reported but not gated (allocator and runner noise).
8. SHOULD state the verification for perf-sensitive changes in the PR (`just perf-baseline` on the base, `just perf-compare` on the branch).
9. MUST NOT commit tests marked `#[ignore]` without an issue or comment explaining when they run.

## Enforcement

| Rule | Mechanism |
|---|---|
| 1, 2, 4, 5 | CI `test` job (`cargo test` with the platform's crate selection) on Linux x64/arm64, Windows x64/arm64, macOS. |
| 3 | Fixture design; reviewers check new process spawns use it. |
| 6, 7 | CI `perf` job: measures the base commit and the PR on the same runner and fails on >10 % regressions (`crates/xtask/src/metric.rs`, tolerance and absolute slack per metric). |
| 2 | `metric.rs`/policy tests use manual clocks; review for real sleeps. |
| 8, 9 | Review checklist. |

## Anti-patterns

- **Gating on RSS.** Identical runs varied by more than 100 % for small tabs because of allocator reuse; the gate flagged a false regression. Fix: gate the deterministic core estimate, report RSS.
- **Comparing numbers across backends.** A deliberate switch to Ghostty read as a snapshot "regression". Fix: results carry `backend`; the gate is skipped when it differs.
- **Hard-coding sshd options from one OpenSSH version** (`PerSourcePenalties` broke on Ubuntu's OpenSSH 9.6). Fix: detect the version, as the fixture now does.
- **A test that passes because it never ran** (an empty Ghostty test run on a platform where the crate is excluded). Fix: CI prints the crate selection; review the job logs when adding platforms.

## References

- ARCH-BRIEF §6 (NFR targets) — design set; `openspec/specs/product-foundation` NFR-00-*.
- `plans/reports/perf-261124-1028-terminal-core-baseline.md`, `plans/reports/ghostty-spike-261124-backend-comparison.md`.
- `justfile` recipes: `test`, `test-ssh`, `term-test`, `perf`, `perf-baseline`, `perf-compare`, `compare-backends`.
