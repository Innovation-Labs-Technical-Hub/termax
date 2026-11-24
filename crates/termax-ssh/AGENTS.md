# termax-ssh

## Purpose

`termax-ssh` is the SSH client facade the rest of Termax talks to. It wraps russh but keeps it private, so the backend can be patched or replaced without changing callers. It opens connections, directly or through `ProxyJump` hops, with a hardened crypto policy. It verifies every host key through a pluggable trust store, authenticates with an ordered list of methods, and exposes exec, interactive shell and local port forwarding (`-L`) channels on one shared transport. Everything runs on tokio, and CPU-heavy steps (key exchange, signing, key decryption) run on the blocking pool, so a tokio runtime is required.

## Public API entry points

- `Session::connect(&ConnectOptions, &ConnectContext)`, and on the cloneable `Session` handle: `exec`, `open_shell`, `forward_local`, `negotiated`, `state`, `subscribe`, `closed` and `disconnect`.
- `ConnectOptions` (with secure `Default`), `JumpHop`, `AuthMethod`, `CryptoPolicy`, `PtyRequest`, and the `DEFAULT_*`, `MIN_WINDOW_SIZE`, `MIN_PACKET_SIZE` and `MAX_PACKET_SIZE` constants.
- Trust: `HostKeyVerifier` (`MemoryVerifier`, `VaultVerifier`), `TrustDecision` (`FixedTrust`), `TrustChoice`, `HostKeyStatus`, `HostKeyInfo`, `host_port`.
- Prompts: `Prompter`, `NoPrompter`, `KbdInteractiveRequest`, `KbdPrompt`.
- Channels: `ShellChannel` (split into `ShellReader` and `ShellWriter`), `ExecOutput`, `ExitStatus`, `Signal`, `ForwardHandle`.
- State and errors: `ConnectionState`, `CloseReason`, `Error`, `AuthAttempt`, `AttemptOutcome`. Also `quirks_for`, `QUIRKS` and `is_post_quantum`.

## Commands

- `cargo test -p termax-ssh` or `just test termax-ssh` (unit tests plus integration tests against a real sshd).
- `just test-ssh` runs only `tests/sshd.rs` with `--nocapture`, which prints the negotiated algorithms.
- `cargo clippy -p termax-ssh --all-targets -- -D warnings`.
- `just sshd` starts a throwaway sshd for manual testing (see `termax-sshd-fixture`).

## Layout

- `src/session.rs`: connect flow (direct and jump hosts), the `Session` handle and keepalive state. `src/handler.rs`: the russh handler (host-key check and algorithm capture).
- `src/options.rs`: options, validation and the crypto policy. `src/quirks.rs`: the server quirk table.
- `src/auth.rs`: method sequencing, prompts and the trivial-auth guard. `src/deadline.rs`: a connect deadline that excludes time spent waiting on the user.
- `src/trust.rs`: verifiers and the unknown-key decision. `src/channel.rs`: shell channels. `src/forward.rs`: `-L` forwarding. `src/error.rs`: the error type.
- `tests/sshd.rs` and `tests/common/`: end-to-end tests against the fixture sshd.

## Conventions

- russh types must not appear in the public API.
- `ConnectOptions` is validated before any I/O. Keep new options range-checked there.
- Quirk rows only describe protocol facts about a server family. They can never weaken the crypto policy or host-key checking, and they only take effect after key exchange.

## Gotchas

- Strict KEX (`kex-strict-c-v00@openssh.com`, the Terrapin countermeasure) is always offered, and `Negotiated::strict_kex` reports whether the server agreed. The default policy prefers `mlkem768x25519-sha256`, then curve25519. SHA-1 `ssh-rsa` signatures are never produced or accepted.
- Trust rules fail closed. An unknown key asks `TrustDecision` and is saved only on `TrustChoice::AcceptAndSave`. Changed and revoked keys always abort without prompting. Any verifier error aborts. If a host suddenly presents a different key type while another key is trusted, that counts as changed, not unknown. Each `ProxyJump` hop is verified separately.
- A server that reports authentication success without verifying anything is refused with `Error::TrivialAuthRefused`, unless the caller sets `ConnectOptions::allow_trivial_auth`, which is off by default.
- `ShellWriter::send_break` (and `ShellChannel::send_break`) always returns `Error::Unsupported`, because russh cannot send the RFC 4335 `break` request. The method exists so callers are written against the final API.
- Integration tests skip (they return early and print `SKIP:`) when `/usr/sbin/sshd` or `ssh-keygen` is missing, so a green run without sshd proves little.
- The crate depends on `termax-vault` for `VaultVerifier`. `VaultStore` is synchronous, so the verifier runs each query on the blocking pool.

## OpenSpec

- `connectivity`, module CO-02 (`FR-CO02-*`): russh facade, strict KEX, PQ-first KEX, rsa-sha2, auth orchestration, trivial-auth refusal, host-key hooks, ProxyJump and the quirks table.
- `connectivity`, module CO-03 (`FR-CO03-*`): channels, window and packet sizes, `-L` forwarding and keepalive. Implementation is partial: there is no remote or dynamic forwarding, proxies, agent forwarding or X11. Check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
