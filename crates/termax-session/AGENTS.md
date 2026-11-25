# termax-session

## Purpose

`termax-session` is the one-process, many-session runtime every client (UI, CLI, agents) talks to. A `SessionRuntime` owns one multi-threaded tokio runtime (2 I/O workers by default, independent of the session count) and the registry of every session. It opens SSH sessions through `termax-ssh`, tracks their lifecycle with validated state transitions and stable `TMX-<AREA>-NNN` error codes, reconnects transient failures with backoff, duplicates sessions onto their existing connection, resolves names asynchronously with Happy Eyeballs, and closes everything gracefully on quit. Each session parses its output on its own task through a bounded, window-sized queue into a `TermCore`, and publishes frames through a triple buffer.

## Public API entry points

- `SessionRuntime::new(RuntimeConfig)`, then `open(OpenRequest) -> SessionHandle`, `duplicate(id, DuplicateMode)`, `session(id)`, `list()`, `subscribe()`, `attach(id, ClientRef, Grant)`, `quit_summary()`, `quit()`, `block_on`, `handle`, `io_workers`, and `service()`.
- `SessionService` (IF-04-01, cloneable, used by `termax-control`): the same registry calls plus `detach`, `quit` (async), `clock`, and `network_changed() -> ProbeReport`.
- `SessionHandle`: `id`, `info`, `state`, `watch_state`, `write`, `resize`, `signal`, `send_break`, `subscribe_output` (raw output tap), `forward_local`, `close`, `with_term`, `take_frame_reader`.
- Records and events: `SessionInfo`, `SessionId`/`ConnectionId` (UUIDv7), `Target`, `ClientRef`/`ClientKind`, `Grant`, `Attachment`, `ChannelInfo`, `SessionEvent`/`EventKind`, `Notice`, `QuitSummary`, `QuitReport`.
- Lifecycle: `SessionState`, `StateKind::can_transition_to`, `state::check_transition`.
- Errors: `codes::{ErrorCode, CATALOGUE, Disconnect, Lang, SuggestedAction, Retry, from_ssh}`; the user-facing pages are in `docs/error-codes.md`. `SessionError` covers API misuse only; session failures are states.
- Policy and plumbing: `ReconnectPolicy`, `Clock` (`SystemClock`, `ManualClock`), `net::{Resolver, SystemResolver, resolve, happy_eyeballs}`, `pipeline::{spawn, PipelineConfig, FrameReader, PipelineInput}`, `log::{ConnectionLog, FileLog, MemoryLog, NullLog}`, `netwatch` (OS network-change and resume triggers), `default_term`.

## Commands

- `cargo test -p termax-session` or `just test termax-session`: unit tests, plus `tests/sshd.rs`, `tests/network.rs` (probes, handshake limit, signals, output tap) and `tests/responsiveness.rs` against the fixture sshd (they print `SKIP:` and pass without `/usr/sbin/sshd`).
- `cargo clippy -p termax-session --all-targets -- -D warnings`.
- `cargo xtask perf` reports "echo under flood … via termax-session" (gated) and "duplicate local overhead (termax-session)" (SSH, reported).

## Layout

- `src/runtime.rs`: `SessionRuntime`, `RuntimeConfig`, the registry and `SessionHandle`. `src/actor.rs`: the per-session task (connect, serve, reconnect, duplicate, close) and the trust wrappers that report `Authenticating`.
- `src/state.rs`: states and the transition table. `src/codes.rs`: the error catalogue and mapping from `termax_ssh::Error`. `src/backoff.rs`: reconnect schedule.
- `src/net.rs`: resolver trait, DNS timeout, Happy Eyeballs, TCP socket options. `src/pipeline.rs`: queue, parser task, triple buffer, idle compaction.
- `src/model.rs`: ids, registry records, events. `src/log.rs`: connection log sinks. `src/clock.rs`: injectable time.
- `src/netwatch.rs`: `if-watch` interface events (debounced 100 ms) and resume detection (wall vs monotonic clock every 10 s while a connection is open), both calling `SessionService::network_changed`.

## Conventions

- Time goes through `RuntimeConfig::clock`; tests use `ManualClock` for backoff, DNS timeouts and idle compaction. Never call `tokio::time` directly for behaviour that a test must control.
- Callers' threads never block: `open`, `duplicate`, `write`, `resize`, `close` only enqueue. `quit` and `block_on` block and must not run inside async code.
- Every `Disconnected` carries an `ErrorCode`. A new failure mode gets a new catalogue row and a section in `docs/error-codes.md` (a unit test checks both); never renumber or reuse a code. Retry class lives in the row: host-key, authentication and configuration codes are `Retry::Never`.
- The connection log and tracing carry states, codes and `user@host:port` only; never secrets or terminal content.

## Gotchas

- russh sends `WINDOW_ADJUST` when data arrives, not when the parser consumes it. The window-sized queue therefore applies back-pressure by not reading the channel; once russh's per-channel buffer fills, its session loop and TCP stop too, which briefly also stalls sibling channels on a shared connection.
- OpenSSH reports `MaxSessions` as `connect failed` (code 2), not "administratively prohibited"; Duplicate falls back to a new connection on any refused channel open.
- `Stalled` is emitted only by `network_changed` probes (no reply within `probe_timeout`, 5 s), followed by `Disconnected(TMX-NET-005)` and reconnect; periodic keepalive misses still surface only as the final timeout.
- `max_parallel_handshakes` (4) bounds TCP connect through shell open for every session, fresh opens included; a permit is held while authentication waits for a prompt.
- sshd refuses `signal` requests for forced commands, so the signal test starts the fixture without `ForceCommand`. `send_break` is `Unsupported` (russh cannot send RFC 4335 breaks).
- Resume detection relies on the monotonic clock stopping during sleep (Linux, macOS); on Windows only network events and keepalives notice a resume. `RuntimeConfig::watch_os_events = false` turns the watchers off (the network tests do, so OS events cannot add probes).
- Ghostty's `memory_estimate` does not change after compaction; RSS does (see the perf harness). On macOS and Windows (alacritty backend) `on_idle` is a no-op.
- Tests force `/bin/sh` through the fixture's `ForceCommand` so the developer's login shell and rc files cannot drop typeahead or rewrite titles.
- `slow_handshake_never_blocks_the_caller_or_other_tabs` asserts the 4 ms bound on the caller's thread at p99 and < 100 ms for every call (shared CI runners preempt threads for 4–12 ms; a real block on the 150 ms-per-packet handshake still fails). It is therefore not tagged as verifying FR-CO01-001 acceptance 2's strict "never > 4 ms". It lives in its own test binary so other tests cannot preempt it.

## OpenSpec

- `connectivity`, module CO-01: FR-CO01-001…005, 009, 010 and FR-CO01-011 (change `add-session-runtime`); FR-CO01-008 network-change and sleep/wake handling (change `add-control-endpoint`). The control endpoint and headless mode (FR-CO01-006, 007) live in `termax-control`. The FR-CO01-011 quiet-period default (60 s) is provisional: [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead).

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/testing-and-performance.md](../../.principles/testing-and-performance.md), [openspec/AGENTS.md](../../openspec/AGENTS.md), [docs/error-codes.md](../../docs/error-codes.md).
