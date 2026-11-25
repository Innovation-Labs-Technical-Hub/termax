# Design

## Context

See proposal.md for scope. Existing building blocks:

- `termax-ssh`: `Session::connect` (strict KEX, host-key trust, auth, keepalive with `ConnectionState`/`subscribe()`), cloneable handles, `exec`, `open_shell` (pty channel), `forward_local`, ProxyJump.
- `termax-term`: `TermCore` with budgeted `feed` returning bytes consumed, `snapshot` into `Frame` + `Damage`, `take_events`, `on_idle`; backends alacritty (all platforms) and Ghostty (Linux default).
- `termax-vault`: hosts, resolved settings, known hosts; `termax-core`: domain types and settings resolver.

Dependency direction today: `termax-ssh → termax-vault → termax-core`. A runtime that drives SSH and terminals therefore cannot live inside `termax-core` without a cycle.

## Goals / Non-Goals

**Goals:**
- One process-wide I/O runtime and session registry used by every client (UI, CLI, agents) — ARCH-BRIEF §4–§5.
- Bounded memory and back-pressure from the network to the parser — ARCH-BRIEF §5, analysis/02-terminal-emulation.md §7 "Threading and flow".
- Deterministic tests: state machine, backoff and idle timing under a mockable clock.

**Non-Goals:**
- Wire protocol for the local control endpoint (FR-CO01-006) — this slice exposes an in-process API that the endpoint will wrap.
- Rendering, panes and UI-thread scheduling (UI-04, D1).

## Decisions

1. **New crate `termax-session`** depending on `termax-core`, `termax-ssh`, `termax-term` (and `termax-term-ghostty` on Linux). Alternatives: put the runtime in `termax-core` (creates a cycle with `termax-ssh`), or in `termax-ssh` (mixes terminal concerns into the transport facade). ARCH-BRIEF §3 lists the orchestrator under `oxt-core`; the split keeps `termax-core` a dependency-free domain crate and is recorded here as a deliberate deviation.
2. **Runtime ownership**: `SessionRuntime` owns one multi-threaded tokio runtime (2 workers by default) and a single timer wheel; no periodic ticks — timers are armed only for keepalive, backoff and idle compaction (ARCH-BRIEF §5, NFR-00-002). KEX and signing already run inside `termax-ssh` tasks; DNS uses the blocking pool or an async resolver with a 5 s timeout (FR-CO01-009).
3. **Data path per session** (ARCH-BRIEF §5; analysis/02 §7): channel reader → bounded queue sized to the SSH window (default 2 MiB, configurable 1–4 MiB) → parser task calling `TermCore::feed` with the default `Budget` (64 KiB / 2 ms) → consumed bytes are returned as window credit → latest `Frame` published through a triple buffer the UI pulls at vsync. The perf harness already measures echo latency under flood for 1 and 4 MiB windows (p99 18–48 ms), which bounds the window choice.
4. **State machine and errors** (FR-CO01-003): an explicit enum with transition validation; every `Disconnected` carries a `TMX-<AREA>-NNN` code from a static catalogue with VI/EN messages, mapped from `termax_ssh::Error` variants. The catalogue is data (one table) so CI can check every code has documentation.
5. **Reconnect** (FR-CO01-004): backoff 1, 2, 4, 8, 16, 32 s capped at 60 s with ±20 % jitter, 10 attempts by default; classification of retryable errors lives next to the error-code catalogue so host-key changes and auth failures are never retried.
6. **Duplicate** (FR-CO01-005): reuse the cloned `termax_ssh::Session` handle to open a new pty channel; on a channel-open failure that indicates MaxSessions, open a new connection and emit a notice event.
7. **Idle compaction** (FR-CO01-011): the parser task records the time of the last fed byte; a single timer per session fires after the configured quiet period and calls `TermCore::on_idle` once until new output arrives. The quiet-period default is an open decision; the setting exists from day one.
8. **Clock injection**: all timing (backoff, idle, StateChanged latency checks) goes through a clock trait, mirroring `termax-policy`'s `Clock`, so tests are deterministic.

## Risks / Trade-offs

- **Window size vs. echo latency**: larger windows raise throughput but delay echo under flood (measured 47.5 ms p99 at 4 MiB on alacritty). Default 2 MiB; revisit with real UI frame pacing.
- **Happy Eyeballs** adds connection-race complexity; if no maintained crate fits the licence policy, implement RFC 8305 minimally (two staggered attempts) and test with an unreachable AAAA fixture.
- **Platform split**: compaction is a no-op on macOS/Windows until the Ghostty build works there; NFR-00-003 is only met on Linux in this slice.
- **MaxSessions detection** depends on how servers report channel-open failure. Verified against OpenSSH 10.2 with `MaxSessions 1` in the sshd fixture: OpenSSH answers with `SSH_OPEN_CONNECT_FAILED` (2), not "administratively prohibited" (1). The fallback therefore triggers on any refused channel open (`termax_ssh::Error::ChannelRefused`, whatever the reason code) and on a connection that closed in the meantime.
- **Window credit**: russh 0.65 sends `WINDOW_ADJUST` when data arrives, not when the application consumes it, and offers no hook to defer it. The parser still returns consumed bytes as credit to the window-sized queue; when the queue is full the reader stops reading the channel, russh's per-channel buffer fills and its session loop (and TCP) stall. Back-pressure holds, but a flooding tab can briefly stall sibling channels on a shared connection. True SSH-level credit needs a russh API change.
- **Stalled**: the state exists and its transitions are validated, but `termax-ssh` reports only the final keepalive timeout, so the runtime does not emit `Stalled` yet.

## Defaults chosen in implementation

Values the decisions above do not fix, chosen and documented here (none is a business decision):

| Setting | Default | Why |
|---|---|---|
| Reconnect jitter | ±20 % uniform, applied after doubling, result capped at 60 s | Decision 5. |
| Reconnect scope | Only sessions that reached `Open`; a first connection that fails stays `Disconnected` | FR-CO01-004 lists failures of a running session; retrying a wrong address silently helps nobody. |
| Happy Eyeballs Connection Attempt Delay | 250 ms | RFC 8305 §5 recommended value; the system resolver returns both families in one answer, so there is no separate resolution delay. |
| Socket options | `SO_KEEPALIVE`, `TCP_NODELAY`, 256 KiB receive buffer | FR-CO01-009 (64–256 KiB). |
| Queue size | the connection's SSH window, accepted range 2–4 MiB | `termax-ssh` refuses windows below 2 MiB (FR-CO03 "≥ 2 MiB initial window"), so decision 3's lower bound of 1 MiB is not reachable for SSH sessions. |
| Frame publish interval while output flows | 8 ms (plus a publish whenever the queue drains) | Half a 60 Hz frame, so the UI always pulls a fresh frame. |
| Blocking-pool cap | 16 threads | Bounds the runtime's thread count (FR-CO01-001); DNS, key decryption and log flush are short. |
| Event buffer per subscriber | 1024 events; a slower subscriber gets `Lagged` and re-reads `list()` | |
| Channel-end grace | 500 ms to see whether a shell that ended without an exit status was a dropped connection | |
| Quit budget | 2 s for closing sessions and flushing the connection log | FR-CO01-010. |
| Idle-compaction quiet period | 60 s, a setting (`RuntimeConfig::idle_compaction_quiet_period`) | **Provisional** [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead); approved as a provisional default by the project owner on 2026-11-25 pending the Tech lead's decision. |
