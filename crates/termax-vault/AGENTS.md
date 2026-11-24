# termax-vault

## Purpose

`termax-vault` is the encrypted local store shared by the desktop app and the `tmx` CLI. It keeps vaults, nested groups, hosts and trusted host keys in one SQLite database (WAL mode, foreign keys on), and it never writes a secret to disk in plaintext. Unlocking needs both the user's password and a 128-bit Secret Key that is never stored. The key hierarchy, record format and schema policy are documented in the crate docs at the top of `src/lib.rs`. Read them before changing anything in `crypto.rs`, `store.rs` or `schema.rs`.

## Public API entry points

- `VaultStore`: `create`, `create_with_params`, `open`, `change_password`, `change_password_with_params`, `kdf_params`, `schema_version`.
- CRUD on `VaultStore`: `create_vault`, `list_vaults`, `create_group`, `delete_group(id, cascade)`, `create_host`, `list_hosts(vault_id, &HostFilter)`, `resolved_settings`, `upsert_known_host`, `get_known_host`, `list_known_hosts`, and the matching get, update and delete methods.
- `SecretKey` (`generate`, `parse`, `to_formatted`), `generate_secret_key`, `KdfParams`, `KnownHost`, `HostFilter`, `SCHEMA_VERSION`, `Error`, `Result`.

## Commands

- `cargo test -p termax-vault` or `just test termax-vault`.
- `cargo clippy -p termax-vault --all-targets -- -D warnings`.
- `just demo-init` creates a throwaway vault under `target/demo/` with a fixed demo password, and `just demo hosts list` runs `tmx` against it.

## Layout

- `src/crypto.rs`: Argon2id and HKDF key derivation, XChaCha20-Poly1305 `seal` and `open`, the `Aad` builder, key wrapping, and the `SecretKey` text format.
- `src/store.rs`: `VaultStore`, unlock and password change, and the known-hosts sub-keys.
- `src/schema.rs`: `PRAGMA user_version` and the append-only `MIGRATIONS` list.
- `src/vaults.rs`, `src/groups.rs`, `src/hosts.rs`, `src/known_hosts.rs`: one table each. Each module's header says which columns are plaintext and why.
- `tests/vault_store.rs`: on-disk properties (no plaintext secrets, ciphertext swap and index tamper detection, migrations, password change) using cheap `KdfParams`.

## Conventions

- Every encrypted column is `version || 24-byte nonce || ciphertext`. The AAD starts with the domain tag `termax-vault` and the version byte, followed by length-prefixed table, column, record id and every plaintext index column of that row. When you add a plaintext column, bind it into that row's AAD.
- KDF and HKDF labels are fixed strings: `termax-vault account-key v1`, `termax-vault known-hosts enc v1`, `termax-vault known-hosts mac v1`, and the header id `argon2id-v19`. Changing any of them makes existing stores unreadable.
- Migrations are append-only: entry `i` upgrades version `i` to `i + 1`. Never edit a released migration. All pending migrations run in one transaction on open. A schema change also needs an OpenSpec delta (see `openspec/AGENTS.md`).
- Mutating methods take `&mut self`, and each one runs in its own SQLite transaction.
- Use `KdfParams::default()` in production. Cheap parameters are for tests only.

## Gotchas

- Wrong credentials must surface as `Error::WrongCredentials`, never as `Error::Corrupt`, because unwrapping the store key is the credential check. A database newer than this build fails with `Error::SchemaTooNew`.
- Host address, port, tags and settings live in one encrypted JSON blob (`settings_ct`). Tag and text filters decrypt the vault's candidate rows in memory, so do not add a deterministic tag index.
- Known hosts are keyed by an HMAC token of `host:port` (`host_port_mac`), not by the clear host name. Look them up through `get_known_host`. `upsert_known_host` keeps the original `first_seen`.
- Changing the password re-wraps the store key and vault keys under a new salt. Record ciphertexts are not touched.
- `delete_group` and `delete_vault` refuse non-empty targets unless `cascade` is true.

## OpenSpec

- `identity-keys-trust`, module CO-08 (`FR-CO08-*`): local SQLite WAL store, per-record encryption, Argon2id plus Secret Key, schema versioning, and the inheritance resolver.
- `identity-keys-trust`: FR-CO10-003 (known-hosts store). Implementation is partial; check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
