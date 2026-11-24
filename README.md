# Termax

Termax is a local-first, agent-native SSH workspace, rebuilt from scratch in Rust. Licence: Apache-2.0.

The design set (C4 architecture, BRD-00…13, module catalogue, mockups) lives outside this repo in `Termax_Plan.zip`. Implementation slices are tracked under [`plans/`](plans/); measurements and interop results are in [`plans/reports/`](plans/reports/).

## Names

| Thing | Name |
|---|---|
| Product and desktop app | Termax (`termax`, not built yet) |
| Command-line tool | `tmx` |
| Rust crates | `termax-*` |
| CLI environment variables | `TMX_VAULT_PATH`, `TMX_VAULT_PASSWORD`, `TMX_SECRET_KEY` |

## Workspace

| Crate | Purpose |
|---|---|
| `termax-core` | Domain model (vaults, groups, hosts) and layered settings resolver (App → Vault → Group → Host → Session) |
| `termax-policy` | Allow/ask/deny engine for agent-proposed shell commands, kill switch, rate limits, hash-chained audit log, output sanitiser |
| `termax-interop` | `~/.ssh/config` and `known_hosts` import, ssh_config export |
| `termax-vault` | Encrypted local store (SQLite WAL + per-record XChaCha20-Poly1305, Argon2id + Secret Key) |
| `termax-ssh` | SSH client facade over russh: strict KEX + ML-KEM, host-key trust, auth, multiplexed channels, `-L`, ProxyJump |
| `termax-term` | `TermCore` terminal-core trait with the alacritty_terminal backend |
| `termax-term-ghostty` (+ `-sys`) | Experimental `TermCore` backend on libghostty-vt; needs Zig 0.15.2, not a default member |
| `termax-cli` | The `tmx` command-line tool |
| `termax-sshd-fixture` | Throwaway unprivileged OpenSSH sshd for tests and the perf harness |
| `xtask` | Perf harness (`cargo xtask perf`) and `cargo xtask sshd` |

## Develop

Tasks are in the [`justfile`](justfile); `make <recipe>` forwards to the same recipes (`make test ARGS=termax-ssh`).

```sh
just              # list recipes
just doctor       # show toolchain versions (Rust, MSRV, sshd, Zig, cargo-deny)
just setup        # rustup components, MSRV toolchain, cargo-deny
just ci           # fmt check, clippy -D warnings, tests, cargo-deny — same as CI
just test termax-ssh
just perf         # terminal + SSH performance against the brief's targets
just perf-baseline && just perf-compare   # local regression check
```

Playground:

```sh
just policy "echo hi && sudo rm -rf /" prod   # ask the agent policy about a command
just demo-init                                # throwaway vault with your ~/.ssh/config imported
just demo hosts list
just sshd                                     # throwaway sshd + ready-to-paste ssh command
```

Ghostty backend:

```sh
just zig-install       # Zig 0.15.2 into ~/.local, SHA-256 verified
just ghostty-test
just ghostty-compare   # alacritty vs Ghostty numbers side by side
```

Plain `cargo build`, `cargo test` and `cargo clippy` use the default workspace members, which skip the Zig-based crates; avoid `--workspace` unless Zig is installed. CI runs the same checks on Linux, Windows and macOS (amd64 and arm64), a separate Ghostty job, `cargo-deny` for the licence and advisory policy in `deny.toml`, and a perf job that fails on >10 % regressions against the PR's base commit.
