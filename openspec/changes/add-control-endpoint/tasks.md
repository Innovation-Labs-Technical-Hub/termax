# Tasks

## 1. Runtime additions (termax-ssh, termax-session)

- [x] 1.1 `SessionService` cloneable handle (IF-04-01) with `open`, `duplicate`, `session`, `list`, `subscribe`, `attach`, `detach`; `SessionRuntime` delegates — verified by the existing `termax-session` tests passing unchanged and by `termax-control`, which uses only `SessionService`.
- [x] 1.2 `SessionHandle::signal`, `send_break` and `subscribe_output` (raw output tap) — verified by `tests/network.rs::signals_reach_the_remote_process_and_the_output_tap_sees_raw_bytes` (KILL ends `exec sleep 30` with exit signal KILL; the marker arrives through the tap). `send_break` returns `Unsupported` (russh limitation, already documented in `termax-ssh`).
- [x] 1.3 `termax_ssh::Session::ping` (`keepalive@openssh.com` want-reply) and `SessionService::network_changed()` probing every connection at once; no reply within 5 s → `Stalled` → `Disconnected(TMX-NET-005)` → reconnect — FR-CO01-008 acceptance 1: `tests/network.rs::network_change_probes_every_connection_and_reconnects_dead_ones` (black-holed proxy, keepalives off, probe sent ≤1 s, Stalled after 5–8 s, reconnects once the path is back; the healthy connection stays Open).
- [x] 1.4 Handshake limit (FIFO, 4 by default, TCP connect through shell open) — FR-CO01-008 acceptance 2: `tests/network.rs::resume_with_twenty_sessions_runs_at_most_four_handshakes_at_once` (20 sessions stall and reconnect; never more than 4 between Connecting and Open, and at least 2 in parallel).
- [x] 1.5 OS network-change watcher (`if-watch`, 100 ms debounce) and resume detection (clock gap while a connection is open) — verified by `netwatch::tests::{bursts_of_events_fire_once_after_the_quiet_period, a_wall_clock_jump_beyond_the_gap_is_a_sleep}`. OS event delivery itself needs a real network change or suspend: manual check, not automated.

## 2. Control endpoint (termax-control)

- [x] 2.1 Paths, per-user directory 0700 with owner check, socket 0600, peer-UID check, single-instance lock; Windows named pipe with owner-only DACL (cfg-gated) — verified by `paths::tests::*`, `tests/endpoint.rs::socket_and_directory_are_private_and_a_second_runtime_is_refused` and a local `cargo check`/`clippy` of the transport for `x86_64-pc-windows-msvc`. Status: done on Linux; open until Windows and macOS CI run the crate. Cross-user rejection (FR-CO01-006 acceptance 1) is not automated: it needs a second OS account. Verified 2026-11-25 by CI run 45633094085 (commit 93c73da): Linux x64/arm64, Windows x64/arm64, macOS, deny (0BSD allowed for interprocess), perf, coverage and spec all green. Known Windows gap (pipe DACL grants the owner, not the user SID; client does not verify the pipe server's owner) is tracked as a follow-up.
- [x] 2.2 Framing (length prefix, 1 MiB limit) and JSON-RPC 2.0 types with `TMX-CTL-*` codes documented in `docs/error-codes.md` — verified by `frame::tests::*`, `protocol::tests::*`, `error::tests::codes_round_trip_and_every_code_is_documented`.
- [x] 2.3 Pairing with approval hook, 256-bit tokens, hashed client registry (0600), revocation — FR-CO01-006 acceptance 2: `tests/endpoint.rs::unpaired_client_is_unauthorized_until_pairing_is_approved`; FR-PL04-012 acceptance 1: `revoked_client_gets_an_auth_error_and_guidance_to_pair_again`; `denied_or_unanswered_pairing_issues_no_token`; `clients::tests::*`, `token::tests::*`.
- [x] 2.4 Scopes and gates; observe-only `send_input` → `TMX-CTL-403` plus an approval request when policy allows; approving it sends the input once — FR-CO01-006 acceptance 3: `tests/endpoint.rs::observe_only_send_input_is_refused_with_403_and_queues_an_approval` (a policy-denied input gets no request; the approved input reaches the shell; the approval does not carry over). `PolicyEngine::rules_verdict` added in `termax-policy` with a test.
- [x] 2.5 Methods `session.list`, `session.open`, `session.attach` (stream), `session.subscribe` (stream), `session.send_input`, `session.special`, every call in the audit chain, rate limits — verified by `tests/endpoint.rs::{operate_client_opens_lists_attaches_types_and_follows_events_over_the_socket (FR-CO01-002 acceptance 1 over the endpoint), mcp_clients_get_sanitised_text_instead_of_raw_bytes, protocol_errors_rate_limits_and_the_audit_chain, shutdown_removes_the_socket}`.
- [x] 2.6 Client-side token store in the OS keychain (`keyring`) — verified by `token_store::tests::keychain_store_round_trips_where_a_keychain_exists` (skips without a keychain) and `memory_store_round_trips`.

## 3. Headless mode and CLI

- [x] 3.1 Headless runner: lock, idle exit after the configured time with no sessions and no clients, socket removed — FR-CO01-007 acceptance 2: `tests/headless.rs::headless_exits_when_idle_and_removes_its_socket` (manual clock, 300 s) and `crates/termax-cli/tests/cli.rs::headless_serves_a_private_socket_refuses_unpaired_clients_and_exits_when_idle` (real process, 3 s).
- [x] 3.2 `tmx headless`, `tmx pair`, `tmx sessions list|open` (`tmx ls`), global `--endpoint` — verified by the CLI test above (socket 0600 in a 0700 directory, unpaired `sessions list` exits with `TMX-CTL-401` and pairing guidance, pairing denied without a terminal, second runtime refused, idle exit) and FR-CO01-007 acceptance 1 at library level: `tests/headless.rs::headless_runtime_opens_a_vault_host_and_lists_it` (vault host, vault known hosts) plus `unknown_host_keys_are_rejected_in_headless_mode`. `tmx ssh` itself (the scenario's trigger) is FR-PL04-003, out of scope.

## 4. Docs and gates

- [x] 4.1 `crates/termax-control/AGENTS.md`, `crates/termax-session/AGENTS.md`, `crates/termax-ssh/AGENTS.md`, `crates/termax-cli/AGENTS.md`, README and root `AGENTS.md` crate maps, `openspec/config.yaml` stack, `docs/error-codes.md` — reviewed in the diff.
- [x] 4.2 Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `just spec-validate`, `cargo xtask spec-coverage --run` with 0 dangling tags. Status: all pass locally on Linux; open until CI runs `cargo deny check` (not installed locally) and the macOS/Windows jobs. Verified 2026-11-25 by CI run 45633094085 (commit 93c73da): Linux x64/arm64, Windows x64/arm64, macOS, deny (0BSD allowed for interprocess), perf, coverage and spec all green.

## Workflow follow-up

- Decide the [TO CONFIRM · CHỜ XÁC NHẬN] items in proposal.md (scope model; pairing approval without a UI) and Q-04-01.
- Archive the change after merge.
