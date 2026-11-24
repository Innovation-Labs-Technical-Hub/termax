# P0 spikes: SSH facade, TermCore, perf harness

Status: in progress (2026-11-24)
Source: `Termax_Plan.zip` → ARCH-BRIEF §2/§5/§6, analysis/01-ssh-core.md §6, analysis/02-terminal-emulation.md §7.
Builds on: `plans/261124-0957-p1-foundation-crates/`.

## Outcome
Real SSH connections through an OxTerm-owned facade over russh, a `TermCore` trait backed by alacritty_terminal that produces Frame + Damage, and a perf harness that measures the NFR numbers, all independent of the UI framework decision (D1).

## Phases
| # | Module | Crate | Status |
|---|---|---|---|
| 0 | MSRV raised to 1.89 (russh ≥0.64.1 security floor requires it) | root | done |
| 1 | SSH transport/auth/crypto policy, host-key trust, exec/shell/pty, keepalive, local forwarding, ProxyJump; interop tests vs OpenSSH 10.2 | `oxt-ssh` | in progress |
| 2 | TermCore trait + alacritty_terminal backend: budgeted feed, replies, key/mouse/paste/focus encoders, resize, scroll, selection, search, Frame + Damage, events | `oxt-term` | in progress |
| 3 | Perf harness: parser MB/s, snapshot cost, RSS per terminal, SSH exec/throughput vs local sshd, budgets as a CI gate | `xtask` | pending |

## Acceptance criteria
- fmt, clippy `-D warnings`, `cargo test --workspace` pass; SSH interop tests run against a real unprivileged sshd and skip with a clear message when sshd is absent.
- Strict KEX required (Terrapin); host-key mismatch aborts; trivial auth refused.
- Parser throughput and per-tab memory reported against the ARCH-BRIEF §6 targets (≥300 MB/s, ≤3 MB per extra 80×24 tab with 10k lines).

## Not in this slice
SFTP, agent forwarding, remote/dynamic forwarding, X11, GSSAPI, FIDO, renderer/UI.
