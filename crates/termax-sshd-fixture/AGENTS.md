# termax-sshd-fixture

## Purpose

`termax-sshd-fixture` starts a throwaway, unprivileged OpenSSH `sshd` on `127.0.0.1` as the current user, with fresh host and client keys in a temporary directory. The `termax-ssh` integration tests, the `cargo xtask perf` SSH measurements, and `cargo xtask sshd` / `just sshd` use it. Shipping code never uses it, and it is `publish = false`.

## Public API entry points

- `Sshd::start() -> Option<Sshd>`. It returns `None`, after printing `SKIP: ...`, when `/usr/sbin/sshd` or `ssh-keygen` is missing.
- On `Sshd`: `opts()` (`ConnectOptions` for the authorized client key, with quick timeouts), `trusting_verifier()`, `authorize(priv_path)`, `rotate_host_key()`, `host_fingerprint()`, `host_port()`, `addr()`, `host_key_path()`, and the `client_key` field.
- Helpers: `key_auth`, `ctx`, `trusted_ctx`, `keygen`, `pubkey_line`, `free_port`, `current_user`, `have`, and the `SSHD` constant.

## Commands

- `cargo test -p termax-sshd-fixture` (unit test of the OpenSSH version parser).
- `cargo clippy -p termax-sshd-fixture --all-targets -- -D warnings`.
- `just test-ssh` exercises the fixture through the `termax-ssh` integration tests.
- `just sshd` (the same as `cargo xtask sshd`) runs it in the foreground and prints an `ssh` command line. Stop it with Ctrl-C, which also removes the temporary directory.

## Layout

- `src/lib.rs`: the whole crate (sshd_config generation, spawn with port retries, key helpers, and connect-context helpers).

## Conventions

- The fixture depends on `termax-ssh` for its option and trust types, so it cannot be a dependency of `termax-ssh` itself. It is a dev-dependency there.
- Keep the generated `sshd_config` minimal and loopback-only: password and keyboard-interactive auth are off, `StrictModes no`, `UsePAM no`, `LogLevel VERBOSE`.

## Gotchas

- The binary path is hard-coded to `/usr/sbin/sshd`. An sshd elsewhere on `PATH` is not found, and every dependent test silently skips.
- `PerSourcePenalties no` is written only when `sshd -V` reports OpenSSH 9.8 or newer. Tests fail authentication on purpose and reconnect quickly, which newer sshd would penalise, while older sshd rejects the unknown keyword.
- `free_port` picks a port and then releases it, so a race is possible. `start` therefore retries the spawn several times.
- Cleanup runs on drop. `cargo xtask sshd` stops on Ctrl-C or SIGTERM and removes the `/tmp/tmx*` key directory; a SIGKILL skips cleanup and leaves both sshd and the directory behind.

## OpenSpec

- No capability of its own. It supports test evidence for `connectivity` (CO-02, CO-03) and the interop lab in `platform-engineering` (FR-PL01-010).

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
