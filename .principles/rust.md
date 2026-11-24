# Rust principles

Scope: every crate under `crates/` (the Cargo workspace), plus `tools/` scripts that generate project files.

## Principles

1. **Crates follow the module map, and dependencies point one way.** Domain (`termax-core`) knows nothing about transport, storage or UI; adapters depend on the domain, never the reverse. A one-way graph keeps the core reusable by desktop, CLI, mobile (FFI) and the cloud. (ARCH-BRIEF §3 "Cargo workspace"; `openspec/config.yaml` stack.)
2. **Facades own third-party crates.** `termax-ssh` hides russh, `termax-term` hides the terminal emulators behind `TermCore`. Upstream churn (about 20 russh advisories a year, an unstable libghostty API) must stay inside one crate so a swap or patch is local. (ARCH-BRIEF §2.)
3. **Errors are values with stable meaning.** Libraries return typed errors; user-visible failures carry stable `TMX-<AREA>-NNN` codes with VI/EN text. Callers branch on variants, never on message strings. (`openspec/specs/connectivity` FR-CO01-003.)
4. **Nothing blocks the caller's thread.** Network, crypto, DNS and parsing run on the tokio runtime or the blocking pool with budgets, so the UI thread stays responsive. (ARCH-BRIEF §5; NFR-00-001.)
5. **Small, explicit public APIs.** Every public item is documented and used by at least one test; internal modules stay `pub(crate)`.

## Rules

1. MUST target edition 2024 and the workspace MSRV (`rust-version` in the root `Cargo.toml`, currently 1.90). Raising it needs a reason in the commit body (e.g. a security floor such as russh ≥ 0.64.1).
2. MUST inherit `[lints] workspace = true`. `unsafe_code = "forbid"` applies everywhere except `termax-term-ghostty-sys`, which holds the only `unsafe` (one `unsafe impl Send` with a written soundness argument). New `unsafe` needs its own `-sys` crate, a `// SAFETY:` comment per block and review by the security owner.
3. MUST keep the dependency direction: `termax-core` depends on no other workspace crate; `termax-ssh` and `termax-vault` never depend on `termax-term`; binaries (`termax-cli`, `xtask`) are leaves. Cycles are resolved by a new crate, not by moving code into `termax-core` (see `openspec/changes/add-session-runtime/design.md` decision 1).
4. MUST declare shared dependencies once in `[workspace.dependencies]` when two or more crates use them; versions of security-relevant crates (russh, libghostty-vt, rusqlite, chacha20poly1305, argon2) are pinned or floored explicitly with a comment explaining why.
5. MUST use `thiserror` enums for library errors and `anyhow` only in binaries and xtask. No `Box<dyn Error>` in public signatures.
6. MUST NOT `unwrap()`/`expect()` on fallible input in library code; allowed in tests, in `xtask`, and for internal invariants where the `expect` message states the invariant.
7. MUST NOT block inside async code: no `std::thread::sleep`, `std::fs` on hot paths or synchronous DNS in tasks; use `tokio::task::spawn_blocking` (as `VaultVerifier` does) or async equivalents.
8. MUST make time injectable wherever behaviour depends on it (rate limits, backoff, idle timers): take a clock trait like `termax_policy::Clock` so tests run on a manual clock.
9. SHOULD keep modules under ~600 lines and split by responsibility (see `termax-term-ghostty/src/{grid,search,selection}.rs`).
10. MUST format with `cargo fmt` and keep `cargo clippy --all-targets -D warnings` clean on the platform's crate selection (`just lint`). Do not silence a lint with `#[allow]` without a comment saying why.
11. MUST name files in snake_case (Rust convention) and crates `termax-<area>`; the CLI binary is `tmx`, env vars `TMX_*`.
12. MUST NOT put plan, phase or finding IDs in code comments; state the invariant. Requirement IDs (FR-/NFR-) belong in test docs, commit bodies and PRs.
13. SHOULD log through `tracing` with structured fields; never format secrets, keys, passwords or terminal content into log messages (see `security.md`).

## Enforcement

| Rule | Mechanism |
|---|---|
| 1, 2, 10 | CI `test` job on 5 platforms: `cargo fmt --all --check`, `cargo clippy … -D warnings`; `unsafe_code = "forbid"` in `[workspace.lints.rust]` fails compilation. `just msrv` checks the MSRV toolchain locally. |
| 3 | Compile-time: a reverse dependency would create a cycle or an unused-crate warning; reviewers check `Cargo.toml` diffs. Planned: `cargo-deny` `[bans]` wrappers or a `cargo metadata` check in xtask. |
| 4 | `cargo-deny` (`deny` job) for advisories, yanked crates and licences; review for pins. |
| 5, 7, 13 | Review checklist (PR template). |
| 6, 13 | `clippy::unwrap_used`, `clippy::expect_used` and `clippy::print_stdout` (`warn`, so `-D warnings` fails CI) in `[workspace.lints.clippy]`; `clippy.toml` exempts tests. An invariant `expect` carries `#[allow(clippy::expect_used, reason = "…")]` at the narrowest scope; binaries (`termax-cli`, `xtask`), examples and test support opt out at the crate root with a reason. Libraries log through `tracing`, never stdout. |
| 8 | Unit tests that use a manual clock (e.g. `termax-policy` rate-limit tests). |
| 11, 12 | Review. |

## Anti-patterns

- **Moving code into `termax-core` to escape a cycle.** It turns the domain crate into a grab bag. Fix: a new crate (as planned for `termax-session`).
- **Matching on error text** (`err.to_string().contains("timeout")`). It breaks on upstream wording changes. Fix: add a variant or map at the facade boundary.
- **Leaking upstream types** (`russh::…`, `alacritty_terminal::…`) through public APIs. Fix: wrap them in Termax types inside the facade crate.
- **Literal OxTerm names.** The design set uses the codename; the repo uses Termax, `termax-*`, `tmx`, `TMX_*`, `TMX-`. The rename once missed `CARGO_BIN_EXE_oxt` and a lowercase test prefix; search case-insensitively when renaming.
- **Silencing clippy after an MSRV bump** (`collapsible_if` fired when let-chains became available). Fix the code instead.

## References

- ARCH-BRIEF §2 (stack), §3 (crates), §5 (threading) — design set `Termax_Plan.zip`.
- `openspec/specs/platform-engineering` (PL-01 build and quality gates), `openspec/specs/connectivity` (CO-01 runtime, error codes).
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/
