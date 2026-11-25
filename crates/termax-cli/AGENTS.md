# termax-cli

## Purpose

`termax-cli` builds `tmx`, the Termax command-line tool. It shares the encrypted local vault with the desktop app. Today it can create and inspect a vault, import hosts and host keys from OpenSSH files, list hosts and export them as ssh_config, dry-run the agent approval policy, run a headless runtime, pair with a runtime and list or open its sessions over the local control endpoint. It is thin glue: parsing and conversion live in `termax-interop`, storage and crypto in `termax-vault`, policy in `termax-policy`, and the endpoint, pairing and headless runner in `termax-control`. Keep business logic out of this crate.

## Public API entry points

This is a binary crate (`[[bin]] name = "tmx"`, `src/main.rs`). Its interface is the command tree:

- `tmx vault init` and `tmx vault info`.
- `tmx import ssh-config [PATH] [--dry-run]` and `tmx import known-hosts [PATH] [--dry-run]`.
- `tmx hosts list [--search TEXT] [--tag TAG] [--env ENV]` and `tmx hosts export`.
- `tmx policy check [--env none|dev|staging|prod] [--tag T] [--rules FILE] [--external ID [--operate]] <COMMAND>`. It exits 0 for allow, 2 for ask and 3 for deny.
- `tmx headless [--idle-timeout SECS]` (default 300): runtime + endpoint without a UI, using the vault for hosts and known hosts; asks pairing and operate approvals on its terminal (denies without one); exits when idle or on Ctrl-C and removes its socket.
- `tmx pair [--operate]`: request pairing (observe, plus operate with the flag); the token goes to the OS keychain under `tmx@<endpoint>`.
- `tmx sessions list` (also `tmx ls`) and `tmx sessions open <HOST> [--cols N --rows N]`. Auth errors print `TMX-CTL-401` and a hint to run `tmx pair`; no runtime prints a hint to start one.
- Global flags: `--db PATH` (or `TMX_VAULT_PATH`), `--endpoint DIR` (or `TMX_ENDPOINT`, the control endpoint directory) and `--json`. Credentials come from `TMX_VAULT_PASSWORD` and `TMX_SECRET_KEY`, or from interactive prompts.

`cargo run -q -p termax-cli -- <subcommand> --help` is the authoritative flag reference.

## Commands

- `cargo test -p termax-cli` or `just test termax-cli`.
- `cargo clippy -p termax-cli --all-targets -- -D warnings`.
- `just policy "sudo rm -rf /" prod` dry-runs one command (the recipe ignores the exit status).
- `just demo-init`, then `just demo hosts list`, uses a throwaway vault in the build output directory with a fixed demo password.

## Layout

- `src/main.rs`: the clap tree, the shared `Ctx` (`db`, `json`), `print_json`, and error-to-exit-code mapping (errors exit 1).
- `src/vault_cmd.rs`: database path resolution, credential prompts, `open_store`, and `default_vault` (the first personal vault).
- `src/import_cmd.rs`, `src/hosts_cmd.rs`, `src/policy_cmd.rs`: one subcommand group each. `src/control_cmd.rs`: `headless`, `pair`, `sessions`/`ls` and the terminal approval prompt.
- `tests/cli.rs`: end-to-end runs of the built `tmx` binary with a temporary home and database, including a `tmx headless` lifecycle test (private socket, unpaired refusal, denied pairing without a terminal, single instance, idle exit).

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

- `automation-logging-cli`, module PL-04 (`FR-PL04-*`): FR-PL04-001 (command tree and global flags), FR-PL04-002 (shared vault), FR-PL04-005 (`tmx vault`), FR-PL04-006 (`tmx import`), FR-PL04-009 (JSON output and exit codes), and part of FR-PL04-012 (pairing; client list and revocation UI pending). `tmx ssh`, `sftp`, `mcp`, `log`, `attach` and `send` are not implemented yet, and no command auto-starts a headless runtime.
- `connectivity`, module CO-01: FR-CO01-007 headless mode (`tmx headless`).

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
