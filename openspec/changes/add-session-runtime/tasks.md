# Tasks

## 1. Crate and runtime

- [ ] 1.1 Create `crates/termax-session` (deps: termax-core, termax-ssh, termax-term; termax-term-ghostty on Linux) and add it to the justfile/CI crate selection — verified by `just ci` on all platforms.
- [ ] 1.2 `SessionRuntime` with one multi-threaded tokio runtime (2 workers, configurable) and an injectable clock — FR-CO01-001 acceptance 1: test opens 50 sessions against the sshd fixture and asserts the worker-thread count does not grow.
- [ ] 1.3 Keep all I/O, DNS and parsing off the caller's thread — FR-CO01-001 acceptance 2: tracing span check that a caller thread is never blocked >4 ms during a slow handshake (sshd fixture with an artificial delay proxy).

## 2. Registry and state machine

- [ ] 2.1 Session registry (UUIDv7 ids, host/ad-hoc target, protocol, connection_id, channels, state, owning client, grants) with `list()` and `subscribe()` — FR-CO01-002 acceptance 1 via the in-process API; StateChanged delivered ≤50 ms (acceptance 2, measured with the test clock and a real sshd).
- [ ] 2.2 Lifecycle state enum with validated transitions — FR-CO01-003 specified behaviour: unit tests for every legal and illegal transition.
- [ ] 2.3 `TMX-<AREA>-NNN` error catalogue with VI/EN messages and mapping from `termax_ssh::Error` — FR-CO01-003 acceptance 1 (refused TCP → `TMX-NET-002`); acceptance 2: a test that every catalogue entry has both messages and a doc link.

## 3. Reconnect, duplicate, DNS, close

- [ ] 3.1 Backoff 1/2/4/8/16/32 s capped at 60 s, ±jitter, 10 attempts default, no retry on host-key change or auth failure — FR-CO01-004 acceptances 1–3 with the test clock and the fixture's freezable proxy; host-key change verified with `rotate_host_key`.
- [ ] 3.2 Duplicate opens a new pty channel on the same connection; MaxSessions fallback to a new connection with a notice event — FR-CO01-005 acceptance 1 (same connection_id, no new KEX, ≤20 ms local overhead measured in xtask perf) and acceptance 2 (sshd fixture with `MaxSessions 1`).
- [ ] 3.3 Cancellable async DNS with 5 s timeout and Happy Eyeballs v2 — FR-CO01-009 acceptance 1 (black-hole resolver → `TMX-DNS-001`, cancellation works) and acceptance 2 (unreachable AAAA + good A → IPv4 within ≤300 ms).
- [ ] 3.4 Graceful close (EOF, DISCONNECT on last channel) and quit summary listing sessions and forwards; logs flushed ≤2 s — FR-CO01-010 acceptance 1.

## 4. Terminal data path

- [ ] 4.1 Per-session parser task: bounded queue sized to the SSH window, budgeted `feed`, consumed bytes returned as window credit, frames published through a triple buffer — verified by an xtask perf metric "echo under flood via termax-session" (p99 ≤50 ms, NFR-00-001) gated against the baseline.
- [ ] 4.2 Idle compaction timer calling `on_idle` once per quiet period — FR-CO01-011 scenarios: memory per full 10k-line tab ≤3 MiB on Linux (xtask perf), content/selection/search unchanged (test), no compaction for busy sessions (test clock).

## 5. Spec and docs

- [ ] 5.1 Resolve or record the FR-CO01-011 quiet-period default with the Tech lead; update the delta and remove the [TO CONFIRM · CHỜ XÁC NHẬN] marker once decided.
- [ ] 5.2 Update README crate table and `openspec/config.yaml` stack section; run `openspec validate --all --strict`, then archive the change after merge.
