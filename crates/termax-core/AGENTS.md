# termax-core

## Purpose

`termax-core` holds the domain model that the desktop app, the `tmx` CLI and later the mobile app share: vaults, nested groups, hosts, and the layered settings resolver (App, Vault, Group chain, Host, Session). It has no I/O and no storage. Persistence lives in `termax-vault`, and OpenSSH import/export lives in `termax-interop`. Keep this crate small and dependency-light (serde, uuid, thiserror), because every other crate depends on it.

## Public API entry points

- `model`: `Vault`, `VaultId`, `VaultKind`, `Group`, `GroupId`, `Host`, `HostId`, `Protocol`, `Env`.
- `settings`: `Settings` (a sparse map from setting key to value), `Layer`, `SettingSource`, and `Resolved` (resolved values with per-key provenance, so the UI can show inheritance badges).
- Everything is re-exported from `src/lib.rs`.

## Commands

- `cargo test -p termax-core` or `just test termax-core` (unit tests only).
- `cargo clippy -p termax-core --all-targets -- -D warnings`.
- `just lint` runs format check and clippy for the whole workspace, which is what CI runs.

## Layout

- `src/lib.rs`: crate docs and re-exports.
- `src/model.rs`: id newtypes over `Uuid` and the vault, group and host records.
- `src/settings.rs`: settings layers and the resolver. Unit tests sit in the same files.

## Conventions

- Layers passed to the resolver are ordered least specific first (App to Session). Later layers override earlier ones, and nested groups resolve innermost first.
- Each layer stores only the values it overrides. Do not copy inherited values down into a layer.
- `Env` drives production safety rules in `termax-policy`, and `termax-vault` binds it into the record AAD. Treat changes to its serialized form as a storage and policy contract change.
- The model types, `Settings` and `SettingSource` carry serde derives. Their serialized form ends up inside encrypted vault records, so renaming a field or variant needs a migration plan.

## Gotchas

- `Host::settings` keys written by the ssh_config importer are lower-case ssh keywords with a specific value encoding. See `termax-interop` (`encode_setting`) before reading or writing them by hand.

## OpenSpec

- `identity-keys-trust`: FR-CO08-009 (configuration inheritance resolver).
- `hosts-and-import`: FR-UI02-002 (nested groups with inherited defaults) and FR-UI03-005 (layered inheritance with badges). This crate supplies the model; the UI parts are not implemented here.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
