# P1 foundation crates — first implementation slice

Status: slice complete (2026-11-24); next slice pending decisions below
Source: `~/Documents/Projects/Termax_Plan.zip` → `OxTerm-SSH-Workspace/` (ARCH-BRIEF.md, modules.yaml, BRD-00…13).

## Outcome
A Cargo workspace for OxTerm with the dependency-free foundation crates from the P1 module catalogue,
each with real behaviour and tests, so the SSH/terminal/UI crates (which need the D1 GPUI-vs-Tauri bake-off) can build on them.

## Phases
| # | Module | Crate | Status |
|---|---|---|---|
| 0 | PL-01 workspace, CI matrix, cargo-deny licence policy | root | done |
| 1 | Domain model + settings inheritance (UI-03/CO-08 resolver) | `oxt-core` | done |
| 2 | AG-03 policy & approval engine (deny > ask > allow, per-sub-command AST, non-overridable rules, prod always asks, hash-chained audit) | `oxt-policy` | done |
| 3 | CO-11 interop: `~/.ssh/config` (Include, basic Match) + `known_hosts` import, ssh_config export | `oxt-interop` | done |
| 4 | CO-08 vault: SQLite WAL, per-record XChaCha20-Poly1305, Argon2id key wrap, migrations | `oxt-vault` | done |
| 5 | PL-04 `oxt` CLI: `vault init|info`, `import ssh-config|known-hosts`, `hosts list|export`, `policy check` | `oxt-cli` | done |

## Acceptance criteria
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` pass.
- No plaintext secret written to disk by `oxt-vault`; wrong password fails closed.
- Policy: `rm -rf /` inside any compound/subshell is denied even with an allow rule; prod hosts never auto-allow.

## Not in this slice
SSH transport (russh), TermCore/renderer, GPUI UI, MCP, sync, mobile. Open decisions D1–D9 are untouched.

## Result
102 tests pass; fmt and clippy (`-D warnings`) clean. Known follow-ups:
- `oxt-policy`: wrappers (`sudo`, `timeout`, …) are stripped before matching, so user rules cannot target them; audit tail truncation is only detectable if `head_hash()` is persisted separately.
- `oxt-interop`: code writing `Host.settings` must use `encode_setting` for faithful export; tags/env/group are not exported to ssh_config.
- `oxt-vault`: no OS-keychain caching yet; `rusqlite` resolved to 0.40.x because of `rust-version = 1.85`.
- `oxt import known-hosts` skips hashed, `@cert-authority` and `@revoked` lines (the vault has no marker column yet).

## Next slice (blocked on D1 bake-off for UI work)
P0 spikes: `oxt-ssh` russh facade + interop matrix, `oxt-term` TermCore over alacritty_terminal, perf harness.
