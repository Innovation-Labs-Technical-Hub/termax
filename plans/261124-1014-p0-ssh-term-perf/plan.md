# P0 spikes: SSH facade, TermCore, perf harness

Status: slice complete (2026-11-24); two perf targets missed, decision needed
Source: `Termax_Plan.zip` → ARCH-BRIEF §2/§5/§6, analysis/01-ssh-core.md §6, analysis/02-terminal-emulation.md §7.
Builds on: `plans/261124-0957-p1-foundation-crates/`.

## Outcome
Real SSH connections through an Termax-owned facade over russh, a `TermCore` trait backed by alacritty_terminal that produces Frame + Damage, and a perf harness that measures the NFR numbers, all independent of the UI framework decision (D1).

## Phases
| # | Module | Crate | Status |
|---|---|---|---|
| 0 | MSRV raised to 1.89 (russh ≥0.64.1 security floor requires it) | root | done |
| 1 | SSH transport/auth/crypto policy, host-key trust, exec/shell/pty, keepalive, local forwarding, ProxyJump; interop tests vs OpenSSH 10.2 | `termax-ssh` | done |
| 2 | TermCore trait + alacritty_terminal backend: budgeted feed, replies, key/mouse/paste/focus encoders, resize, scroll, selection, search, Frame + Damage, events | `termax-term` | done |
| 3 | Perf harness: parser MB/s, snapshot cost, RSS per terminal, SSH exec/throughput vs local sshd, budgets as a CI gate | `xtask` | done (CI report-only) |

## Acceptance criteria
- fmt, clippy `-D warnings`, `cargo test --workspace` pass; SSH interop tests run against a real unprivileged sshd and skip with a clear message when sshd is absent.
- Strict KEX required (Terrapin); host-key mismatch aborts; trivial auth refused.
- Parser throughput and per-tab memory reported against the ARCH-BRIEF §6 targets (≥300 MB/s, ≤3 MB per extra 80×24 tab with 10k lines).

## Not in this slice
SFTP, agent forwarding, remote/dynamic forwarding, X11, GSSAPI, FIDO, renderer/UI.

## Result
185 tests pass; fmt and clippy clean. Reports: `plans/reports/termax-ssh-261124-interop-matrix.md`, `plans/reports/perf-261124-1028-terminal-core-baseline.md`.
- Met: snapshot costs, new-channel latency, SSH throughput (1.5× OpenSSH client on loopback), strict KEX + ML-KEM negotiated with OpenSSH 10.2.
- Missed: parser 158 MB/s vs ≥300 (raw alacritty is 177 MB/s, so the backend is the limit); 18.9 MiB per 10k-line tab vs ≤3 MiB (alacritty's 24-byte cells).
- Gaps: russh has no BREAK request; sntrup761 not offered; password/kbd-interactive and trivial-auth refusal are unit-tested only; Windows agent pipe not compiled.

## Follow-up (2026-11-24 afternoon)
- Renamed OxTerm → Termax: crates `termax-*`, CLI `tmx`, env `TMX_*`, Secret Key prefix `TX1-`, vault KDF/AAD labels `termax-vault` (vaults created before the rename cannot be opened; none exist outside tests).
- Perf harness v2 (targets vs regressions, flood echo latency, memory tiers) and CI base-vs-head regression gate.
- Ghostty backend spike (`termax-term-ghostty`), CI job with pinned Zig 0.15.2, `just ghostty-compare`.
- `justfile` + forwarding `Makefile` for toolchains, checks, perf and a playground (`demo-*`, `policy`, `sshd`).
- rusqlite 0.43 (SQLite 3.54.1) for RUSTSEC-2026-0459.
