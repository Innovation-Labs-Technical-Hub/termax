//! Termax SSH client facade.
//!
//! The application talks only to the types in this crate; the SSH
//! implementation (russh) stays private so it can be patched or replaced.
//!
//! - [`Session::connect`] opens a connection (optionally through
//!   [`JumpHop`]s), verifies each host key through a [`HostKeyVerifier`] and
//!   [`TrustDecision`], and authenticates with the [`AuthMethod`]s in order.
//! - A [`Session`] is a cloneable handle: [`Session::exec`],
//!   [`Session::open_shell`] and [`Session::forward_local`] open channels on
//!   the same transport.
//! - Keepalives use `keepalive@openssh.com` with want-reply; when too many go
//!   unanswered the state switches to
//!   [`ConnectionState::Closed`]`(`[`CloseReason::KeepaliveTimeout`]`)`.
//!
//! # Security properties
//!
//! - Strict key exchange (`kex-strict-c-v00@openssh.com`, Terrapin
//!   countermeasure) is always offered; [`Negotiated::strict_kex`] reports
//!   whether the server agreed.
//! - The default [`CryptoPolicy`] prefers `mlkem768x25519-sha256`, then
//!   curve25519; AEAD ciphers; ed25519/ECDSA/`rsa-sha2-*` host keys. SHA-1
//!   (`ssh-rsa`) signatures are never produced or accepted.
//! - Changed or revoked host keys always abort; verifier errors abort.
//! - A server that reports authentication success without having verified
//!   anything is refused ([`Error::TrivialAuthRefused`]).
//! - Key exchange, signing and key decryption never block the caller's
//!   thread: everything runs on tokio, with CPU-heavy steps on the blocking
//!   pool. A tokio runtime is required.

mod auth;
mod channel;
mod deadline;
mod error;
mod forward;
mod handler;
mod options;
mod quirks;
mod session;
mod trust;

pub use auth::{KbdInteractiveRequest, KbdPrompt, NoPrompter, Prompter};
pub use channel::{ExitStatus, ShellChannel, ShellReader, ShellWriter, Signal};
pub use error::{AttemptOutcome, AuthAttempt, ChannelOpenFailure, Error, Result};
pub use forward::ForwardHandle;
pub use options::{
    AuthMethod, ConnectOptions, CryptoPolicy, DEFAULT_CIPHER, DEFAULT_HOST_KEY, DEFAULT_KEX,
    DEFAULT_MAC, JumpHop, MAX_PACKET_SIZE, MIN_PACKET_SIZE, MIN_WINDOW_SIZE, PtyRequest,
};
pub use quirks::{QUIRKS, QuirkEntry, Quirks, quirks_for};
pub use session::{
    CloseReason, ConnectContext, ConnectionState, ExecOutput, Negotiated, Session, is_post_quantum,
};
pub use trust::{
    BoxFuture, FixedTrust, HostKeyInfo, HostKeyStatus, HostKeyVerifier, MemoryVerifier,
    TrustChoice, TrustDecision, VaultVerifier, host_port,
};
pub use zeroize::Zeroizing;
