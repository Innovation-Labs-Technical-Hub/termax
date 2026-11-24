# Termax — agent and contributor guide

Termax is a local-first, agent-native SSH workspace written in Rust (product codename in the design set: OxTerm). This file is the entry point for AI agents and humans; crate-level `AGENTS.md` files add local detail.

## Stack

Rust 2024, workspace MSRV 1.90, tokio. SSH via a russh 0.65 facade; terminal cores behind the `TermCore` trait (libghostty-vt via Zig 0.15.2 on Linux, alacritty_terminal elsewhere); encrypted SQLite vault (rusqlite 0.43, XChaCha20-Poly1305, Argon2id + Secret Key); tree-sitter-bash command policy; clap CLI `tmx`. Planned: GPUI desktop (decision D1 pending), rmcp MCP, wasmtime plugins, Flutter mobile, Rust cloud service (separate repos).

## Directory map

| Path | What | Owner (role) |
|---|---|---|
| `crates/termax-core` | Domain model, layered settings resolver | Tech lead |
| `crates/termax-policy` | Allow/ask/deny engine for agent commands, audit chain, output sanitiser | AI/agentic engineer |
| `crates/termax-interop` | ssh_config and known_hosts import/export | Tech lead |
| `crates/termax-vault` | Encrypted local store | Platform/security |
| `crates/termax-ssh` | SSH client facade (trust, auth, channels, forwarding, ProxyJump) | Tech lead |
| `crates/termax-term`, `termax-term-ghostty(-sys)` | Terminal cores and the shared `TermCore` trait | Terminal engineer |
| `crates/termax-cli` | `tmx` CLI | Tech lead |
| `crates/termax-sshd-fixture`, `crates/xtask` | Test sshd, perf harness, `xtask sshd` | Platform |
| `openspec/` | Requirements (14 capabilities), changes, glossary — see `openspec/AGENTS.md` | Product owner + tech lead |
| `.principles/` | Engineering rules with enforcement | Tech lead |
| `plans/` | Delivery plans and reports (stateful records, not product authority) | Whoever runs the slice |
| `tools/` | Generators (`openspec_from_brd.py`) | Tech lead |

## Golden rules

1. **Spec first.** Behaviour changes go through an OpenSpec change citing FR-/NFR- ids. Never resolve `[TO CONFIRM · CHỜ XÁC NHẬN]` markers or decisions D1–D9 by assumption — ask the owner.
2. **Respect crate boundaries.** `termax-core` depends on no workspace crate; upstream types stay inside their facade crate; `unsafe` only in `termax-term-ghostty-sys`. See `.principles/rust.md`.
3. **Fail closed, no plaintext secrets, agents are untrusted.** See `.principles/security.md`. Never read, print or commit secrets or `.env*` files.
4. **Verify before claiming done.** Run the commands below; report failures with output; never weaken tests, lints or gates.
5. **Licences.** Apache-2.0 core; dependencies must pass `cargo deny check` (`deny.toml`); no GPL linkage.
6. **Branches and PRs.** Never commit to `main`; conventional commits; no plan IDs in code comments. Ask before pushing, merging or other outward-facing actions not already authorized. See `.principles/agentic-coding.md`.
7. **Generated files.** `openspec/specs/**` and `openspec/glossary.md` were generated once from the design set; edit them only through OpenSpec changes. `.claude/skills/openspec-*` and `.claude/commands/opsx/` are managed by `openspec init/update`.

## Commands

Tasks live in the `justfile` (`make <recipe>` forwards to it). Verified on Linux:

```sh
just doctor          # toolchain versions: Rust, MSRV, clippy, fmt, cargo-deny, sshd, Zig
just setup           # rustup components, MSRV toolchain, cargo-deny, Zig 0.15.2
just lint            # cargo fmt --check + clippy -D warnings (platform crate selection)
just test [crate]    # all tests, or one crate: just test termax-ssh
just test-ssh        # SSH interop tests against a throwaway OpenSSH sshd
just term-test       # both TermCore backends against shared expectations
just spec-validate   # openspec validate --all --strict (pinned 1.17.0)
just spec-coverage   # OpenSpec scenarios linked to tests; --phase P1, --run, --json FILE
just perf            # perf harness; perf-baseline / perf-compare for local regression checks
just ci              # lint + test + deny, as CI runs them (needs cargo-deny: just setup)
just sshd            # throwaway sshd with a ready-to-paste ssh command
```

On Linux, building needs Zig 0.15.2 on `PATH`; the first Ghostty build clones Ghostty at a pinned commit (set `GHOSTTY_SOURCE_DIR` for offline builds). On macOS and Windows the Ghostty crates are excluded automatically.

## CI

`.github/workflows/ci.yml`: `test` (fmt, clippy, tests on Linux x64/arm64, Windows x64/arm64, macOS), `spec` (OpenSpec strict validation), `deny` (licences, advisories), `perf` (regression gate against the PR's base commit), `coverage` (`cargo xtask spec-coverage --run`; report only, fails on dangling `Verifies:` tags).

## Links

- Principles: `.principles/README.md`
- Specs and workflow: `openspec/AGENTS.md`, `openspec/config.yaml`
- Plans and reports: `plans/`, `plans/reports/`
- Design set (outside the repo): `Termax_Plan.zip` — ARCH-BRIEF, C4 architecture, BRD-00…13, modules.yaml, mockups
