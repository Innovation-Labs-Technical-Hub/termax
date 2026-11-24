# termax-cli

## Purpose

`termax-cli` builds `tmx`, the Termax command-line tool. It shares the encrypted local vault with the desktop app. Today it can create and inspect a vault, import hosts and host keys from OpenSSH files, list hosts and export them as ssh_config, and dry-run the agent approval policy. It is thin glue: parsing and conversion live in `termax-interop`, storage and crypto in `termax-vault`, and policy in `termax-policy`. Keep business logic out of this crate.

## Public API entry points

This is a binary crate (`[[bin]] name = "tmx"`, `src/main.rs`). Its interface is the command tree:

- `tmx vault init` and `tmx vault info`.
- `tmx import ssh-config [PATH] [--dry-run]` and `tmx import known-hosts [PATH] [--dry-run]`.
- `tmx hosts list [--search TEXT] [--tag TAG] [--env ENV]` and `tmx hosts export`.
- `tmx policy check [--env none|dev|staging|prod] [--tag T] [--rules FILE] [--external ID [--operate]] <COMMAND>`. It exits 0 for allow, 2 for ask and 3 for deny.
- Global flags: `--db PATH` (or `TMX_VAULT_PATH`) and `--json`. Credentials come from `TMX_VAULT_PASSWORD` and `TMX_SECRET_KEY`, or from interactive prompts.

`cargo run -q -p termax-cli -- <subcommand> --help` is the authoritative flag reference.

## Commands

- `cargo test -p termax-cli` or `just test termax-cli`.
- `cargo clippy -p termax-cli --all-targets -- -D warnings`.
- `just policy "sudo rm -rf /" prod` dry-runs one command (the recipe ignores the exit status).
- `just demo-init`, then `just demo hosts list`, uses a throwaway vault in the build output directory with a fixed demo password.

## Layout

- `src/main.rs`: the clap tree, the shared `Ctx` (`db`, `json`), `print_json`, and error-to-exit-code mapping (errors exit 1).
- `src/vault_cmd.rs`: database path resolution, credential prompts, `open_store`, and `default_vault` (the first personal vault).
- `src/import_cmd.rs`, `src/hosts_cmd.rs`, `src/policy_cmd.rs`: one subcommand group each.
- `tests/cli.rs`: end-to-end runs of the built `tmx` binary with a temporary home and database.

## Conventions

- Every command supports `--json` for machine-readable output. Human output and JSON come from the same report struct.
- Read secrets from the environment or `rpassword` prompts only. Never accept a password or Secret Key as a positional argument or flag.
- `vault init` refuses to overwrite an existing database, and prints the Secret Key exactly once.

## Gotchas

- Without `--db` or `TMX_VAULT_PATH`, the database lives in the per-user data directory (`directories::ProjectDirs` for `dev.Termax.termax`). Tests and demos must always pass `--db`.
- `vault init` has a hidden `--kdf-memory-kib` flag for tests and demos. It also lowers the Argon2id time and parallelism costs to 1, so use it only for throwaway vaults. New passwords must have at least 8 characters.
- `import ssh-config` skips hosts whose label already exists, case-insensitively, and reports them as duplicates. `import known-hosts` stores only concrete, plain host entries. Hashed names, wildcard patterns, and `@cert-authority` and `@revoked` lines are reported as skipped.
- `just tmx ...` and `just demo ...` pass each argument through unchanged (positional arguments), so `just tmx policy check "rm -rf /"` works. `tmx policy check` exits 0/2/3 for allow/ask/deny, so `just` reports a non-zero exit as a recipe failure.

## OpenSpec

- `automation-logging-cli`, module PL-04 (`FR-PL04-*`): FR-PL04-001 (command tree and global flags), FR-PL04-002 (shared vault), FR-PL04-005 (`tmx vault`), FR-PL04-006 (`tmx import`), and FR-PL04-009 (JSON output and exit codes). `tmx ssh`, `sftp`, `mcp`, `log` and session attach are not implemented yet.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
