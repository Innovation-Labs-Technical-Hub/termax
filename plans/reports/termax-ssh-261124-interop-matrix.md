# termax-ssh interop matrix: OpenSSH 10.2

Date: 2026-11-24. Backend: russh 0.65.4 (aws-lc-rs). Server: `SSH-2.0-OpenSSH_10.2p1 Ubuntu-2ubuntu3.8`, running unprivileged on 127.0.0.1 from the test harness (`crates/termax-ssh/tests/common/mod.rs`).

## Negotiated with the default `CryptoPolicy`

| Item | Value |
|---|---|
| KEX | `mlkem768x25519-sha256` (post-quantum hybrid, `post_quantum = true`) |
| Strict KEX (Terrapin) | in effect (`Names::strict_kex() == true`) |
| Host key | `ssh-ed25519` |
| Cipher | `chacha20-poly1305@openssh.com` |
| MAC (both directions) | `hmac-sha2-512-etm@openssh.com` (listed by the backend, but not used because the cipher is AEAD) |
| Compression | `none` (pinned) |

`sntrup761x25519-sha512` is not implemented in russh 0.65, so ML-KEM is the only post-quantum KEX we offer.

## Feature results (`cargo test -p termax-ssh`: 20 unit tests and 16 sshd tests)

| Feature | Result |
|---|---|
| publickey (unencrypted ed25519 file) + exec with exit 0 / exit 3 / stderr | pass |
| Encrypted key, passphrase supplied through `Prompter`; a cancelled prompt skips the key | pass |
| ssh-agent (`ssh-agent -D -a <sock>` + `ssh-add`) with session-bind | pass |
| Wrong key: `AuthFailed` lists publickey rejected, missing key unavailable, password and kbd-interactive not offered | pass |
| PTY shell: `TERM` reaches the server; resize, signal, eof and close succeed; exit status 0 is captured | pass |
| Two execs plus a shell running at the same time on one connection | pass |
| Unknown host key: Reject aborts, AcceptOnce does not save, AcceptAndSave saves and the next connect is Trusted with no prompt | pass |
| Saved fingerprint equals `ssh-keygen -l -E sha256` and `termax_interop::fingerprint_sha256` | pass |
| Changed host key (rotated between connects) aborts without asking the user | pass |
| Revoked host key aborts | pass |
| `-L` forward: 100 kB echo round trip twice; a bind conflict gives `ForwardBind`; the listener is gone after close | pass |
| ProxyJump: two hops to itself, with 3 independent host-key checks; an untrusted first hop aborts | pass |
| Keepalive: after the TCP proxy is frozen, the state becomes `Closed(KeepaliveTimeout)` within about 1 s (300 ms interval, 2 misses allowed) | pass |
| Connect timeout against a silent listener | pass |

## Not covered by live interop

- **Trivial-auth refusal.** OpenSSH cannot be configured to accept `none` or an empty keyboard-interactive exchange without PAM, so this is covered by unit tests only (`auth::tests`). Public-key success counts as verified only if our signer actually produced a signature. A success reply to the unsigned probe is refused.
- **Password and keyboard-interactive.** These need PAM or real account passwords for the test user, so they are not exercised. The code paths share the prompt and timeout plumbing with the tested methods.
- **BREAK (RFC 4335).** russh does not expose the `break` channel request. `send_break` returns `Error::Unsupported`.
- **Windows agent pipe.** The code is cfg-gated (`\\.\pipe\openssh-ssh-agent`) and has not been compiled on Windows here.

## Notes

- russh logs at `error` level ("Maximum packet size ... should not larger than a TCP packet") on every connect, because the required 256 KiB default exceeds 65535. The message is harmless: russh's transport limit is sized for 256 KiB channel packets.
- The sshd harness now lives in `crates/termax-sshd-fixture` so the perf harness can reuse it.
