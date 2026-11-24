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
- **MaxSessions detection** depends on how servers report channel-open failure; the fallback is triggered by the generic "administratively prohibited" reason and verified against OpenSSH with `MaxSessions 1` in the sshd fixture.
