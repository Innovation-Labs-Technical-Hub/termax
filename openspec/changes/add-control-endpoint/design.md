# Design

## Context

`termax-session` (change `add-session-runtime`) owns one tokio runtime and the session registry, with an in-process API (`SessionRuntime::open/list/subscribe/attach/…`). Every session failure is a state with a `TMX-<AREA>-NNN` code; reconnect with backoff exists; `Stalled` is in the state machine but never emitted. `termax-ssh` sends periodic `keepalive@openssh.com` and reports only the final timeout. `termax-policy` provides the command policy engine, a SHA-256 hash-chained `AuditLog` and `sanitize_output`. `tmx` is a synchronous clap binary that opens the vault directly. There is no OS keychain integration and no IPC anywhere in the workspace.

Security rules that shape this design: `.principles/security.md` rule 1 (no plaintext tokens on disk outside the vault), 3 (agents are untrusted callers), 6 (agent commands through the policy engine, output to models through `sanitize_output`), 7 (tool data is data) and 12 (listeners are local only).

## Goals / Non-Goals

**Goals:**
- Reach the runtime from other processes of the same OS user only, with per-client pairing the user approves, scopes, rate limits and an audit trail (FR-CO01-006, NFR-04-009, NFR-07-011).
- Run the runtime without a UI and exit when unused (FR-CO01-007).
- Detect dead connections after network changes and resume within seconds, without reconnect storms (FR-CO01-008).
- Test everything over the real socket against the sshd fixture.

**Non-Goals:**
- The desktop pairing sheet and client list (D1), `tmx ssh/attach/send/mcp`, scrollback, forwards and SFTP over the endpoint (see proposal).
- Cross-user tests: they need a second OS account (root); permission tests check file modes, peer-UID logic and token rejection instead.

## Decisions

1. **New crate `termax-control`** (endpoint transport, JSON-RPC protocol, pairing, client registry, approvals, server, client, headless runner) depending on `termax-session`, `termax-ssh` (signal names) and `termax-policy`. `tmx` stays a leaf binary. Putting IPC into `termax-session` would mix transport, auth and keychain concerns into the runtime (`.principles/rust.md` rule 3; ARCH-BRIEF §3 lists the endpoint beside the orchestrator).
2. **`SessionService` (IF-04-01)**: a cloneable handle on the runtime's shared state (`open`, `duplicate`, `session`, `list`, `subscribe`, `attach`, `detach`, `network_changed`), returned by `SessionRuntime::service()`. The endpoint and future UI both use it; `SessionRuntime` keeps owning the tokio runtime and delegates.
3. **Transport** (ARCH-BRIEF §4; PuTTY `uxshare.c`/`winshare.c`/`winnpc.c` connection sharing):
   - Unix: `<runtime dir>/termax/control.sock`. Runtime dir is `$XDG_RUNTIME_DIR` on Linux and other Unix, `~/Library/Application Support` on macOS. The directory is created 0700 and must be owned by the effective UID with no group/other bits, else the endpoint refuses to start (fail closed). The socket is chmod 0600 right after bind (the 0700 directory closes the window between bind and chmod). Every accepted connection's peer UID (`SO_PEERCRED`/`getpeereid` via tokio `peer_cred`) must equal the effective UID, else it is dropped and a warning is logged. Path length is checked against `sun_path` (104 bytes on macOS, 108 elsewhere) with a clear error.
   - Windows: named pipe `\\.\pipe\termax-control-<hash>` created through `interprocess` (0BSD OR Apache-2.0) with SDDL `D:P(A;;GA;;;OW)` — a protected DACL whose only entry grants the pipe's owner (the creating user) access — and `PIPE_REJECT_REMOTE_CLIENTS` (interprocess default). The client process id is logged. Using `interprocess` keeps the workspace free of `unsafe` (security descriptors otherwise need raw Win32 calls).
   - One runtime per user: an exclusive lock (`std::fs::File::try_lock`, Rust 1.89+) on `control.lock` next to the socket, held for the process lifetime. A stale socket (lock acquired but socket file present) is removed before bind.
4. **Protocol**: each frame is a 4-byte big-endian length followed by a UTF-8 JSON-RPC 2.0 message; frames above 1 MiB close the connection. Batches are refused (`-32600`). The first call is `control.hello {api_version, token?}` or `control.pair`; `api_version` is 1 and a different major version is refused. Errors use JSON-RPC codes (`-32700`, `-32600`, `-32601`, `-32602`) or `-32000` with `data.code` = a stable `TMX-CTL-NNN` code (documented in `docs/error-codes.md`):
   - `TMX-CTL-400` bad request (unsupported `api_version`, malformed params), `TMX-CTL-401` unauthorized (no, unknown or revoked token; pairing denied), `TMX-CTL-403` scope or policy refusal (`data.approval_id` when an approval request was queued), `TMX-CTL-404` unknown session, `TMX-CTL-429` rate limited, `TMX-CTL-500` operation failed (`data.session_code` carries the session's `TMX-*` code when there is one).
5. **Pairing and tokens** (FR-CO01-006, FR-PL04-012, NFR-04-009): `control.pair {name, kind: cli|mcp, scopes}` queues a `Pairing` approval with client name, kind, PID, binary path (Linux `/proc/<pid>/exe`) and requested scopes, then waits for the decision. On approval the server draws 32 bytes from the OS CSPRNG (`getrandom`), returns the token once (hex) and stores only its SHA-256 in the client registry with the granted scopes; the connection is authenticated with them. Tokens are compared by hash with `subtle::ConstantTimeEq`. Clients keep the token in the OS keychain (`keyring`: macOS Keychain, Windows Credential Manager, Linux kernel keyutils); tokens never appear in logs, errors, audit entries or `Debug`. The registry (`control-clients.json`, 0600, per-user config directory) holds id, name, kind, binary path, scopes, token hash, paired/last-seen times and the revoked flag (the `cli_clients` entity, minus `token_ref`, which is the keychain entry on the client). `ControlHandle::revoke` revokes a client; its next `control.hello` gets `TMX-CTL-401` with guidance to pair again.
6. **Approvals hook**: `Approvals` is a queue of `ApprovalRequest { id, kind: Pairing | Operate }`; `ControlHandle::approvals()` lets a UI or CLI subscribe, list pending requests and call `approve(id, scopes)` / `deny(id)`. Nothing is ever approved automatically; an unanswered pairing is denied after 120 s. `tmx headless` implements the hook as a terminal prompt (`[y/N]`, default deny) when stdin is a TTY and denies otherwise.
7. **Scopes and gates** (FR-CO01-006, BR-04-08, IF-04-02): `observe` covers `session.list`, `session.subscribe` and read-only `session.attach`; `operate` (which includes observe) covers `session.open`, interactive `session.attach`, `session.send_input` and `session.special`. An observe-only client calling an operate method gets `TMX-CTL-403`; for `send_input` and `special` an `Operate` approval request is queued when policy allows (for input: the policy's command rules do not deny it). Approving it performs that one call ("approve once", J-04-04). `send_input` from operate clients goes through `PolicyEngine::evaluate` (security rule 6, FR-PL04-008): Allow sends, Ask queues an approval and returns 403, Deny returns 403. `session.special` (signals, break) is not a shell command and is not evaluated by the command rules.
8. **Audit** (AG-03, FR-CO01-006 "every call audited"): the server appends one entry per call to a `termax_policy::AuditLog` (hash chain): actor `external:<client id>` (or `unpaired`), command `<method> [session id]`, verdict Allow/Deny with the error code. Input text, tokens and output never enter this log; the policy engine's own log keeps the command text it evaluates, as it does today.
9. **Streams**: `session.attach` returns a subscription id, then `session.output` notifications carry raw output bytes (base64) to CLI clients; MCP clients get `text` passed through `sanitize_output` (security rule 6). A subscriber that falls behind gets a `lagged` count. `session.subscribe` streams `session.event` notifications (state changes with error codes, title, cwd, bell, exit, reconnected; titles and cwd are sanitised for MCP clients). Attachments are recorded in the registry (`SessionInfo::clients`) and removed when the connection closes. The runtime taps raw output from the channel reader into a bounded broadcast (256 chunks) only while someone is attached.
10. **Headless** (FR-CO01-007): `tmx headless [--idle-timeout SECS]` opens the vault (env or TTY prompt, as other `tmx` commands), starts the runtime and endpoint, and runs until idle for the timeout (default 300 s) with no sessions and no connected clients, or until Ctrl-C; on exit it quits the runtime gracefully and removes the socket and lock. Session targets come from the vault (host label → address, port, `user`, `identityfile`), authentication uses identity files and the ssh-agent, and host keys are checked against the vault's known hosts; an unknown host is rejected (fail closed; the user connects once from the app or imports known_hosts). The idle timer runs on the injectable clock.
11. **Network change and resume** (FR-CO01-008; `pinger.c` analysis HS13): `termax-ssh` gains `Session::ping` (`keepalive@openssh.com` with want-reply, waiting for the reply). `SessionService::network_changed()` pings every distinct live connection concurrently; a connection with no reply within 5 s (`RuntimeConfig::probe_timeout`, runtime clock) moves each of its sessions to `Stalled`, then `Disconnected(TMX-NET-005)`, then reconnects per FR-CO01-004. A FIFO semaphore (`RuntimeConfig::max_parallel_handshakes`, 4) bounds concurrent TCP connect + SSH handshake + authentication across all sessions, so a resume with 20 sessions runs at most 4 at a time.
    - OS events: `if-watch` (MIT OR Apache-2.0, maintained by libp2p) delivers interface address changes on Linux (netlink), macOS (SystemConfiguration) and Windows (`NotifyIpInterfaceChange`); events are debounced for 100 ms and trigger `network_changed()`.
    - Resume: no maintained cross-platform crate exists. While at least one connection is open, a check every 10 s compares wall-clock and monotonic elapsed time; a gap above 5 s means the machine slept and triggers `network_changed()`. This works where the monotonic clock stops during sleep (Linux `CLOCK_MONOTONIC`, macOS); see Risks for Windows.

## Risks / Trade-offs

- **Windows untested locally**: the named-pipe transport is compiled on Windows CI only. The DACL uses the owner-rights SID (`OW`) instead of the literal user SID, and the pipe name hashes `USERDOMAIN\USERNAME` instead of the SID, because reading the token SID needs `unsafe` Win32 calls. For an elevated administrator the default owner is the Administrators group, so other administrators could connect. The client does not verify the pipe server's owner (PuTTY `winnpc.c` does); a same-name pipe squatted by another user before Termax starts would receive the token. Follow-up: a `-sys` crate for the SID and server-owner checks, reviewed by the security owner.
- **Cross-user rejection is not tested automatically**: needs a second account (NFR-04-009 measures it on CI runners with one). Tests cover directory/socket modes, the owner check, peer-UID rejection logic and token rejection.
- **Linux keychain is keyutils**: survives logout for a few days but not reboot; the client then pairs again. Secret Service (D-Bus) would add C dependencies.
- **Resume on Windows**: `Instant` may advance during sleep, so the clock-gap check can miss a resume. Windows relies on `if-watch` network events (Wi-Fi reassociation after resume) and keepalive timeouts until a power-notification hook exists.
- **Handshake limit includes user prompts**: a permit is held while authentication waits for a password prompt, so a fifth session can wait behind a pending prompt.
- **Approvals for operate clients**: with no user policy rules every input is Ask, so operate clients need approval per input until rules are configured; this is the policy engine's documented default.

### Defaults chosen where the spec gives no value

| Setting | Default | Note |
|---|---|---|
| Frame limit | 1 MiB | Larger frames close the connection. |
| `api_version` | 1 | |
| Pairing wait | 120 s, then denied | Default deny. |
| Rate limit | 20 calls/s per connection, burst 40 | `TMX-CTL-429` beyond. |
| Output tap | 256 chunks per subscriber, then `lagged` | |
| Unix fallback dir | `$TMPDIR/termax-<uid>` when `XDG_RUNTIME_DIR` is unset | Same owner and mode checks. |
| Client registry | `control-clients.json` in the per-user config dir, 0600, token hashes only | Not in the vault yet. |
| `session.open` scope | operate | [TO CONFIRM · CHỜ XÁC NHẬN] scope model (owner: Product owner + Tech lead). |
| Headless approvals | terminal prompt, deny without a TTY | [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech lead). |
| Headless idle | 300 s (`--idle-timeout`) | From the spec. |
| Probe timeout / parallel handshakes | 5 s / 4 | From the spec. |
| Network event debounce | 100 ms | Keeps the probe within the 1 s target. |
| Resume check | every 10 s while a connection is open; gap > 5 s | |
