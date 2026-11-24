# P1 foundation crates — first implementation slice

Status: in progress (2026-11-24)
Source: `~/Documents/Projects/Termax_Plan.zip` → `OxTerm-SSH-Workspace/` (ARCH-BRIEF.md, modules.yaml, BRD-00…13).

## Outcome
A Cargo workspace for OxTerm with the dependency-free foundation crates from the P1 module catalogue,
each with real behaviour and tests, so the SSH/terminal/UI crates (which need the D1 GPUI-vs-Tauri bake-off) can build on them.

## Phases
| # | Module | Crate | Status |
|---|---|---|---|
| 0 | PL-01 workspace, CI matrix, cargo-deny licence policy | root | done |
| 1 | Domain model + settings inheritance (UI-03/CO-08 resolver) | `oxt-core` | done |
| 2 | AG-03 policy & approval engine (deny > ask > allow, per-sub-command AST, non-overridable rules, prod always asks, hash-chained audit) | `oxt-policy` | in progress |
| 3 | CO-11 interop: `~/.ssh/config` (Include, basic Match) + `known_hosts` import, ssh_config export | `oxt-interop` | in progress |
| 4 | CO-08 vault: SQLite WAL, per-record XChaCha20-Poly1305, Argon2id key wrap, migrations | `oxt-vault` | in progress |
| 5 | PL-04 `oxt` CLI: `vault init`, `import ssh-config`, `hosts list`, `policy check` | `oxt-cli` | pending |

## Acceptance criteria
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` pass.
- No plaintext secret written to disk by `oxt-vault`; wrong password fails closed.
- Policy: `rm -rf /` inside any compound/subshell is denied even with an allow rule; prod hosts never auto-allow.

## Not in this slice
SSH transport (russh), TermCore/renderer, GPUI UI, MCP, sync, mobile. Open decisions D1–D9 are untouched.
