# OxTerm

OxTerm ("Termax" project; provisional codename) is a local-first, agent-native SSH workspace, rebuilt from scratch in Rust. Licence: Apache-2.0.

The design set (C4 architecture, BRD-00…13, module catalogue, mockups) lives outside this repo in `Termax_Plan.zip`. The current implementation slice is tracked in [`plans/261124-0957-p1-foundation-crates/plan.md`](plans/261124-0957-p1-foundation-crates/plan.md).

## Workspace

| Crate | Purpose |
|---|---|
| `oxt-core` | Domain model (vaults, groups, hosts) and layered settings resolver (App → Vault → Group → Host → Session) |
| `oxt-policy` | Allow/ask/deny engine for agent-proposed shell commands, kill switch, rate limits, hash-chained audit log, output sanitiser |
| `oxt-interop` | `~/.ssh/config` and `known_hosts` import, ssh_config export |
| `oxt-vault` | Encrypted local store (SQLite WAL + per-record XChaCha20-Poly1305, Argon2id + Secret Key) |
| `oxt-cli` | `oxt` command-line tool |

## Develop

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs the same checks on Linux, Windows and macOS (amd64 and arm64), plus `cargo-deny` for the licence policy in `deny.toml` (no GPL code linked).
