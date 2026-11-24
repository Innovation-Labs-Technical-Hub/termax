# termax-interop

## Purpose

`termax-interop` moves data between Termax and OpenSSH client files. It parses `~/.ssh/config` with ssh_config(5) semantics (`Include`, `Host`, basic `Match`, first value wins). It resolves the effective options for an alias the way `ssh -G` does, imports concrete host aliases as `termax_core::Host` records, and exports hosts back to ssh_config text. It also parses `known_hosts` files, including hashed entries and `@cert-authority` and `@revoked` markers, and computes OpenSSH-style SHA256 fingerprints. It only reads and writes text. Storing the results is done by `termax-vault`, driven by `tmx import` in `termax-cli`.

## Public API entry points

- `SshConfig::parse_str` and `SshConfig::parse_file` (with `ParseOptions::new(home_dir, local_user)` or `ParseOptions::from_env()`), then `resolve(alias) -> ResolvedConfig` and `import(vault_id) -> ImportReport`. `warnings()` returns `Warning`s.
- `to_ssh_config(&[Host]) -> String` for export.
- `encode_setting`, `decode_setting` and `MULTI_VALUE_SEPARATOR` for the `Host::settings` value format. `SkippedPattern` reports `Host` patterns that were not imported as hosts.
- `KnownHosts::parse_str`, `KnownHosts::parse_file` and `KnownHosts::lookup`, plus `KnownHostEntry`, `HostSpec`, `Marker` and `fingerprint_sha256`.
- `InteropError`.

## Commands

- `cargo test -p termax-interop` or `just test termax-interop`.
- `cargo clippy -p termax-interop --all-targets -- -D warnings`.

## Layout

- `src/ssh_config/mod.rs`: `SshConfig`, `ParseOptions`, block and condition structure, and `Include` handling.
- `src/ssh_config/lexer.rs`: line tokenisation following OpenSSH `readconf.c` and `argv_split`. `keywords.rs`: the client keyword table.
- `src/ssh_config/resolve.rs`: `ssh -G` style resolution and token expansion. `convert.rs`: import, export and the settings encoding.
- `src/known_hosts.rs`: known_hosts parsing, host matching and fingerprints. `src/pattern.rs`: wildcard matching with `*`, `?` and `!`.
- `tests/ssh_config.rs`, `tests/known_hosts.rs`, and fixtures under `tests/fixtures/` (a fake home with `conf.d` includes, an include loop, and hashed and marked known_hosts lines).

## Conventions

- Imported options are stored in `Host::settings` under the lower-case ssh keyword. `HostName` and `Port` map to `Host::address` and `Host::port` instead. Always go through `encode_setting` and `decode_setting`:
  - Each argument is quoted ssh_config-style when it contains whitespace, quotes, `\` or a leading `#`, and the arguments are joined by single spaces.
  - Command keywords (`proxycommand`, `localcommand`, `remotecommand`, `knownhostscommand`) keep the raw command string unquoted.
  - Repeated directives of a multi-valued keyword (`identityfile`, `certificatefile`, `localforward`, `remoteforward`, `dynamicforward`, `sendenv`) are joined with `MULTI_VALUE_SEPARATOR` (`\n`).
- Stored values stay unexpanded (`~`, `%h` and similar tokens are kept), so that they export back verbatim. Only `resolve` expands tokens.
- Tests inject `ParseOptions` with a fixture home. Never depend on the real home directory in tests.

## Gotchas

- Nothing in this crate ever executes a command found in configuration (`ProxyCommand`, `Match exec`, `KnownHostsCommand`, and so on). Such values stay plain strings. Keep it that way.
- An `Include` inside a `Host` or `Match` block inherits that block's condition. Nesting is capped at `MAX_INCLUDE_DEPTH` (16, as in OpenSSH), and include cycles are detected.
- A `Match` line with an unsupported or invalid criterion never matches. It does not match everything.
- `to_ssh_config` exports only `Protocol::Ssh` hosts. It replaces `*`, `?` and `!` in labels with `_` and drops settings whose key is not an ssh_config keyword. Hosts that share a label shadow each other in ssh.
- `decode_setting` fails on unbalanced quotes in a non-command value.

## OpenSpec

- `hosts-and-import`, module CO-11 (`FR-CO11-*`): FR-CO11-002 (ssh_config with Include and basic Match), FR-CO11-003 (known_hosts), and FR-CO11-008 (export ssh_config). Other CO-11 requirements (PuTTY, Termius, TOML, rollback) are not implemented here yet.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
