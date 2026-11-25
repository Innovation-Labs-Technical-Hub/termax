# Proposal

## Why

`termax-session` gives one process a runtime and registry for every session, but only in-process. The `tmx` CLI, the `tmx mcp` shim and the future desktop app's second-launch path all need to reach that runtime from another process, safely, and the CLI must work when no desktop app is running. Sessions must also survive laptop network changes and sleep/wake instead of hanging until a TCP timeout. These are the remaining CO-01 requirements of phase P1 (ARCH-BRIEF §4 "local control endpoint", §5 runtime; BRD-04).

## What Changes

Implements, in capability `connectivity` (module CO-01), without changing requirement text:

- **FR-CO01-006 Local control endpoint**: length-framed JSON-RPC 2.0 over a Unix domain socket (directory 0700, socket 0600, peer UID checked) or a Windows named pipe (DACL for the current user only, remote clients rejected); per-client 256-bit tokens issued through a pairing flow the user approves (`control.pair`), stored in the OS keychain on the client side; observe and operate scopes; rate limits; every call recorded in a hash-chained audit log (AG-03). Methods: `control.hello`, `control.pair`, `session.list`, `session.open`, `session.attach` (output stream), `session.subscribe` (event stream), `session.send_input` and `session.special` (operate). An observe-only client calling an operate method gets `TMX-CTL-403`, and an approval request is queued when policy allows.
- **FR-CO01-007 Headless mode**: `tmx headless` runs the session runtime and the endpoint without a UI, shares the vault, holds a per-user lock, and exits after 5 minutes (configurable) with no sessions and no clients, removing its socket. `tmx pair` and `tmx sessions list|open` talk to it (or to the app later) through the endpoint.
- **FR-CO01-008 Network-change and sleep/wake handling**: OS network-change events (and resume detection) probe every connection at once with `keepalive@openssh.com` (want-reply); no reply within 5 s marks the session `Stalled`, then it reconnects per FR-CO01-004; at most 4 SSH handshakes run in parallel, the rest queue.
- `termax-session` gains `SessionService`, the cloneable in-process form of IF-04-01, plus signals/break, a raw output tap for attach, detach, connection probes and a handshake limit.
- Related interfaces implemented in part: IF-04-01 SessionService, IF-04-02 local control endpoint, IF-07-04 / IF-08-02 (transport and pairing only).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. The requirements above already exist in `openspec/specs/connectivity/spec.md`; this change implements them without changing their text, so it carries no spec delta (`skip_specs: true`). Interface details the spec leaves open (error codes besides `TMX-CTL-403`, the `control.hello` handshake, framing limits, defaults) are recorded in design.md.

## Out of scope

- IF-04-02 methods `session.read_scrollback`, `forward.list/start/stop`, `connection.info`; IF-07-04 `hosts.*`, `sftp.*`, `transfer.*`, `vault.*`, `log.*`.
- `tmx ssh`, `tmx attach`, `tmx send`, `tmx mcp` (FR-PL04-003, -008, AG-02), and auto-starting a headless runtime from those commands (see the pairing question below).
- Desktop pairing UI SCR-04-08 and Settings ▸ Security client list (UI framework decision D1 [TO CONFIRM · CHỜ XÁC NHẬN]); this change exposes the approval hook they will call.
- Persisting the control audit log (AG-03 audit store); the log is in memory for the runtime's lifetime.
- Q-04-01, how the desktop app takes over live headless sessions [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead).

## Dependencies and open items

- Builds on `add-session-runtime` (branch `feat/session-runtime`).
- Scope model: FR-CO01-006 defines observe/operate, FR-PL04-012 defines read/connect/operate/admin. This change implements observe/operate and gates `session.open` as operate. [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Product owner + Tech lead).
- Who approves pairing when no UI runs: `tmx headless` asks on its own terminal and denies when it has none. [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead) before CLI commands auto-start a background headless runtime.
- Defaults chosen where the spec gives no value are listed in design.md "Risks / Trade-offs".
