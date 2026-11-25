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
| `termax-session` | Session runtime: one I/O runtime and session registry, lifecycle states and `TMX-` error codes ([docs/error-codes.md](docs/error-codes.md)), reconnect with backoff, Duplicate on the same connection, async DNS + Happy Eyeballs, per-session parser task with idle compaction, connection probes on network change and resume |
| `termax-control` | Local control endpoint: JSON-RPC 2.0 over a per-user Unix socket (0600) or named pipe (owner-only DACL), client pairing with user approval and keychain-stored tokens, observe/operate scopes, policy-checked input, audit chain, and headless mode |
| `termax-term` | `TermCore` terminal-core trait, shared input encoders, and the alacritty_terminal backend (kept for comparison and as a fallback) |
| `termax-term-ghostty` (+ `-sys`) | Default `TermCore` backend on Linux, on libghostty-vt (Ghostty's terminal core); built with Zig 0.15.2. macOS and Windows use the alacritty backend until Ghostty's Zig build supports the macOS 26 SDK and Visual Studio 2026 |
| `termax-cli` | The `tmx` command-line tool, including `tmx headless`, `tmx pair` and `tmx sessions` |
| `termax-sshd-fixture` | Throwaway unprivileged OpenSSH sshd for tests and the perf harness |
| `xtask` | Perf harness (`cargo xtask perf`), `cargo xtask spec-coverage` and `cargo xtask sshd` |

## Specs and principles

- Requirements: [`openspec/`](openspec/) — 14 capability specs generated from the BRDs, active changes, and a bilingual glossary; workflow in [`openspec/AGENTS.md`](openspec/AGENTS.md).
- Engineering rules: [`.principles/`](.principles/README.md) — Rust conventions, security, testing and performance, agentic coding.
- Agents and contributors start at [`AGENTS.md`](AGENTS.md).

## Develop

Tasks are in the [`justfile`](justfile); `make <recipe>` forwards to the same recipes (`make test ARGS=termax-ssh`).

```sh
just              # list recipes
just doctor       # show toolchain versions (Rust, MSRV, sshd, Zig, cargo-deny)
just setup        # rustup components, MSRV toolchain, cargo-deny, Zig 0.15.2 (SHA-256 verified)
just ci           # fmt check, clippy -D warnings, tests, cargo-deny — same as CI
just test termax-ssh
just perf         # terminal + SSH performance against the brief's targets
just perf-baseline && just perf-compare   # local regression check
just spec-coverage --phase P1   # OpenSpec scenarios linked to tests (`--run` also runs them)
```

Playground:

```sh
just policy "echo hi && sudo rm -rf /" prod   # ask the agent policy about a command
just demo-init                                # throwaway vault with your ~/.ssh/config imported
just demo hosts list
just sshd                                     # throwaway sshd + ready-to-paste ssh command
```

Terminal backends:

```sh
just term-test          # both backends against the same expectations
just compare-backends   # Ghostty vs alacritty numbers side by side
```

On Linux, building needs Zig 0.15.2 on `PATH` (`just zig-install`); on macOS and Windows the Ghostty crates are skipped by `just` and CI. The first build of the Ghostty backend in a fresh build directory clones Ghostty from GitHub at a pinned commit; set `GHOSTTY_SOURCE_DIR` to a local checkout of that commit to build offline. CI runs the same checks on Linux, Windows and macOS (amd64 and arm64), `cargo-deny` for the licence and advisory policy in `deny.toml`, and a perf job that fails on >10 % regressions against the PR's base commit (skipped when the base used a different terminal backend), and a coverage job that reports OpenSpec scenario coverage and fails on dangling `Verifies:` tags.
