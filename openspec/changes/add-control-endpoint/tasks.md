# Tasks

## 1. Runtime additions (termax-ssh, termax-session)

- [ ] 1.1 `SessionService` cloneable handle (IF-04-01) with `open`, `duplicate`, `session`, `list`, `subscribe`, `attach`, `detach`; `SessionRuntime` delegates — verified by the existing `termax-session` tests passing unchanged plus `service_outlives_borrow_and_lists_sessions`.
- [ ] 1.2 `SessionHandle::signal`, `send_break` and `subscribe_output` (raw output tap) — verified by `tests/sshd.rs` tests that interrupt a running `sleep` with `INT` and read echoed output through the tap.
- [ ] 1.3 `termax_ssh::Session::ping` (`keepalive@openssh.com` want-reply) and `SessionService::network_changed()` probing every connection at once; no reply within 5 s → `Stalled` → `Disconnected(TMX-NET-005)` → reconnect — FR-CO01-008 acceptance 1: test freezes the fixture proxy, calls `network_changed`, asserts every probe was sent within 1 s, the dead session goes Stalled and reconnects after the proxy recovers while the healthy one stays Open.
- [ ] 1.4 Handshake limit (FIFO, 4 by default) — FR-CO01-008 acceptance 2: 20 sessions reconnect after a network change and the event stream never shows more than 4 sessions in Connecting/Handshaking/Authenticating at once.
- [ ] 1.5 OS network-change watcher (`if-watch`, 100 ms debounce) and resume detection (clock gap while a connection is open) — verified by unit tests of the debounce and gap detection; OS event delivery itself is checked manually (needs root or a real network change).

## 2. Control endpoint (termax-control)

- [ ] 2.1 Paths, per-user directory 0700 with owner check, socket 0600, peer-UID check, single-instance lock; Windows named pipe with owner-only DACL (cfg-gated) — verified by `paths` unit tests (mode, foreign-mode directory refused), `tests/endpoint.rs::socket_and_directory_are_private` and a Windows CI build.
- [ ] 2.2 Framing (length prefix, 1 MiB limit) and JSON-RPC 2.0 types with `TMX-CTL-*` codes documented in `docs/error-codes.md` — verified by frame and protocol unit tests and a docs test.
- [ ] 2.3 Pairing with approval hook, 256-bit tokens, hashed client registry (0600), revocation — FR-CO01-006 acceptance 2: `unpaired_client_is_unauthorized_until_pairing_is_approved` over the real socket; revocation test; denial and timeout tests.
- [ ] 2.4 Scopes and gates; observe-only `send_input` → `TMX-CTL-403` plus an approval request when policy allows; approving it sends the input once — FR-CO01-006 acceptance 3: `observe_only_send_input_is_refused_with_403_and_queues_an_approval` against the sshd fixture.
- [ ] 2.5 Methods `session.list`, `session.open`, `session.attach` (stream), `session.subscribe` (stream), `session.send_input`, `session.special`, with every call in the audit chain and rate limits — verified end to end over the socket against the sshd fixture (`tests/endpoint.rs`), plus rate-limit and audit tests; FR-CO01-002 acceptance 1 over the endpoint.
- [ ] 2.6 Client-side token store in the OS keychain (`keyring`) — verified by a round-trip test that skips when no keychain is available.

## 3. Headless mode and CLI

- [ ] 3.1 Headless runner: lock, idle exit after the configured time with no sessions and no clients, socket removed — FR-CO01-007 acceptance 2 with the manual clock (`headless_exits_when_idle_and_removes_its_socket`).
- [ ] 3.2 `tmx headless`, `tmx pair`, `tmx sessions list|open`, global `--endpoint` — verified by `crates/termax-cli/tests/cli.rs` (headless starts, socket is 0600, unpaired `sessions list` exits with an auth error and pairing guidance) and a library-level test of FR-CO01-007 acceptance 1 (headless runtime opens a vault host and `session.list` lists it).

## 4. Docs and gates

- [ ] 4.1 `crates/termax-control/AGENTS.md`, `crates/termax-session/AGENTS.md`, `crates/termax-cli/AGENTS.md`, README and root `AGENTS.md` crate maps — reviewed in the diff.
- [ ] 4.2 Gates: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `just spec-validate`, `cargo xtask spec-coverage --run` with 0 dangling tags. `cargo deny check` and macOS/Windows run in CI.

## Workflow follow-up

- Decide the [TO CONFIRM · CHỜ XÁC NHẬN] items in proposal.md (scope model; pairing approval without a UI) and Q-04-01.
- Archive the change after merge.
