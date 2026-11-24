# Proposal

## Why

The foundation slices delivered SSH connections (termax-ssh), terminal cores (termax-term, termax-term-ghostty) and the encrypted vault, but nothing yet owns a *session*: there is no single place that opens, tracks, duplicates, reconnects and closes sessions for the UI, CLI and agents. Every later P1 module (terminal workspace UI-04, port forwarding UI-06, CLI attach PL-04, MCP server AG-02) depends on that runtime, so it is the next slice on the critical path (ARCH-BRIEF §5, §10 P1).

## What Changes

- Add the session runtime that implements, in capability `connectivity`:
  - FR-CO01-001 One-process, many-session runtime (shared I/O runtime, UI thread never does I/O, crypto, DNS or parsing).
  - FR-CO01-002 Session registry with list/subscribe and `StateChanged` events (in-process API; the control-endpoint form is FR-CO01-006, out of scope).
  - FR-CO01-003 Lifecycle state machine and stable `TMX-<AREA>-NNN` error codes with VI/EN messages.
  - FR-CO01-004 Auto-reconnect with exponential backoff and jitter; no retry on host-key change or auth errors.
  - FR-CO01-005 Duplicate opens a new channel on the existing connection, falling back to a new connection at MaxSessions.
  - FR-CO01-009 Async, cancellable DNS with timeout and Happy Eyeballs v2.
  - FR-CO01-010 Graceful session close and app quit.
- Wire each session's terminal core to the runtime: output is parsed off the UI thread in budgets, and parsed bytes return SSH window credit (ARCH-BRIEF §5; NFR-00-001).
- **ADDED** FR-CO01-011 Idle compaction of terminal history: the runtime asks the terminal core to compact memory after output has been quiet, so a full 10k-line tab meets NFR-00-003 (≤3 MiB per extra tab).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `connectivity`: adds FR-CO01-011 (idle compaction of terminal history). The other requirements listed above already exist in the spec and are implemented by this change without changing their text.

## Out of scope

- FR-CO01-006 local control endpoint, FR-CO01-007 headless mode, FR-CO01-008 network-change and sleep/wake handling (next connectivity slice).
- Desktop UI (UI-01, UI-04): blocked on decision D1 (UI framework) [TO CONFIRM · CHỜ XÁC NHẬN].
- Telnet, serial, raw, local shell and Mosh sessions (CO-07); this slice covers SSH sessions only.

## Dependencies and open items

- FR-CO01-011 quiet period before compaction: [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead). The implementation makes it configurable; the default needs a decision.
- macOS and Windows use the alacritty terminal backend, which has no compaction (see `openspec/config.yaml` open decisions), so FR-CO01-011 only reduces memory where the Ghostty backend is the default.
