# termax-control

## Purpose

`termax-control` is the local control endpoint: the only way other processes of the same OS user (`tmx`, the future `tmx mcp` shim, a second app launch) reach the session runtime. It speaks length-framed JSON-RPC 2.0 over a Unix domain socket in a 0700 per-user directory (socket 0600, peer UID checked) or over a Windows named pipe whose DACL admits only its owner. Clients pair once with the user's approval and then authenticate with a 256-bit token kept in the OS keychain; scopes are observe and operate. It also runs headless mode: the runtime and endpoint without a UI, exiting when idle.

## Public API entry points

- Endpoint location and lock: `Endpoint::{for_current_user, in_dir, socket_path, pipe_name, display}`, `InstanceLock::acquire`, `paths::{prepare_private_dir, check_private_dir, check_private_socket, config_dir, REGISTRY_NAME}`.
- Transport: `transport::{Listener::bind, Listener::accept, connect, Stream, Peer}`.
- Server: `server::start(SessionService, Listener, ServerConfig) -> ControlHandle`; `ServerConfig::new(registry, opener, policy)` with `approval_timeout` and `RateLimit`; `SessionOpener` (builds `session.open` requests from runtime-side data). `ControlHandle::{approvals, clients, revoke, audit_log, connected_clients, watch_connected, shutdown}`.
- Approval hook for the UI: `Approvals::{subscribe, pending, approve, approve_with_scopes, deny}`, `ApprovalRequest`, `ApprovalKind::{Pairing, Operate}`.
- Clients: `ClientRegistry` (hashes only), `ClientRecord`, `Scope`, `ClientKind`, `Token`, `TokenStore` (`KeychainStore`, `MemoryStore`), `ControlClient::{connect, hello, pair, call, call_value, next_notification}`.
- Protocol: `protocol::*` (methods table in the module docs), `CtlCode` (`TMX-CTL-NNN`), `ControlError`, `API_VERSION`.
- Headless: `Headless::{start, control, wait_until_idle, shutdown}`, `VaultOpener`, `DEFAULT_IDLE_TIMEOUT`.

## Commands

- `cargo test -p termax-control` or `just test termax-control`: unit tests, `tests/endpoint.rs` (pairing, scopes, approvals, streams, rate limit, audit over the real socket) and `tests/headless.rs` (idle exit, vault sessions). Session tests use the fixture sshd and print `SKIP:` without it.
- `cargo clippy -p termax-control --all-targets -- -D warnings`.
- `tmx headless`, `tmx pair`, `tmx sessions list|open` exercise it by hand (see `crates/termax-cli/AGENTS.md`).

## Layout

- `src/paths.rs`: directories, owner and mode checks, lock. `src/transport.rs`: UDS (`cfg(unix)`) and named pipe through `interprocess` (`cfg(windows)`).
- `src/frame.rs`: 4-byte length framing, 1 MiB limit. `src/protocol.rs`: JSON-RPC types and method parameters.
- `src/server.rs`: accept loop, connections, rate limit, audit, `control.hello` and `control.pair`. `src/methods.rs`: `session.*` methods and streams.
- `src/clients.rs`, `src/token.rs`, `src/token_store.rs`: registry, tokens, keychain. `src/approvals.rs`: approval queue. `src/headless.rs`: headless runner and `VaultOpener`. `src/error.rs`: codes and errors.

## Conventions

- Fail closed: an endpoint directory not owned by the user is refused; unknown host keys are rejected in headless mode; an unanswered approval is denied after 120 s; nothing is approved automatically.
- Tokens never appear in logs, errors, `Debug`, audit entries or the registry file (only `token_sha256`). Clients store them only through `TokenStore` (OS keychain).
- Every call is audited (`ControlHandle::audit_log`, a `termax_policy::AuditLog` hash chain) with method, session id and outcome only: never input text, output or tokens.
- Input from clients goes through `PolicyEngine` (`evaluate` for operate clients; `rules_verdict` decides whether an observe-only client's refused input may be offered for approval). Output and event text for MCP clients goes through `sanitize_output`.
- New refusal kinds get a `CtlCode` and a `docs/error-codes.md` section (a unit test checks both). Never renumber codes.

## Gotchas

- Unix socket paths are limited to 104 bytes on macOS and 108 elsewhere; tests put endpoints under `/tmp`.
- The Windows transport is only compiled and run on Windows CI. Its DACL uses the owner-rights SID and the pipe name hashes the user name and directory, not the user SID (see design.md of `add-control-endpoint`).
- Linux keychain is kernel keyutils: tokens survive logout for days but not a reboot; `tmx pair` again afterwards.
- Cross-user access cannot be tested without a second account; tests check modes, the owner checks and token rejection.
- `tmx headless` asks approvals on its own terminal and denies them without one; auto-starting it in the background is not implemented (open question in the change proposal).

## OpenSpec

- `connectivity`, module CO-01: FR-CO01-006 (local control endpoint) and FR-CO01-007 (headless mode), change `add-control-endpoint`. Interfaces IF-04-02 (partially: no `read_scrollback`, `forward.*`, `connection.info`), IF-07-04 and IF-08-02 (transport and pairing). Scope model and headless approvals are [TO CONFIRM · CHỜ XÁC NHẬN].

## See also

[.principles/security.md](../../.principles/security.md), [.principles/rust.md](../../.principles/rust.md), [openspec/AGENTS.md](../../openspec/AGENTS.md), [docs/error-codes.md](../../docs/error-codes.md), [crates/termax-session/AGENTS.md](../termax-session/AGENTS.md).
