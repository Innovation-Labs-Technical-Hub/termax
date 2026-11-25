//! Termax session runtime.
//!
//! One [`SessionRuntime`] per process owns a multi-threaded tokio runtime
//! (2 I/O workers by default, independent of the session count) and the
//! registry of every session. UI, CLI and agent clients call into it; it
//! never blocks their threads.
//!
//! - [`SessionRuntime::open`] starts a session that moves through
//!   [`SessionState`]s: resolving (async DNS with a timeout), connecting
//!   (Happy Eyeballs v2), handshaking, authenticating, open. Failures carry a
//!   stable [`ErrorCode`] (`TMX-<AREA>-NNN`) with VI/EN text and suggested
//!   actions ([`codes`]).
//! - Transient failures reconnect with backoff ([`ReconnectPolicy`]); host-key
//!   and authentication failures never do.
//! - [`SessionRuntime::duplicate`] opens a new channel on the same connection,
//!   or a new connection when the server refuses one.
//! - [`SessionRuntime::list`] and [`SessionRuntime::subscribe`] expose the
//!   registry and its [`SessionEvent`]s; [`SessionRuntime::quit`] closes
//!   everything gracefully.
//! - Each session parses output on its own task ([`pipeline`]): a bounded
//!   queue sized to the SSH window, budgeted `TermCore::feed`, frames through
//!   a triple buffer, and idle compaction after a quiet period.
//!
//! All timing goes through an injectable [`Clock`].

mod actor;
pub mod backoff;
pub mod clock;
pub mod codes;
pub mod log;
mod model;
pub mod net;
pub mod pipeline;
mod runtime;
pub mod state;

pub use backoff::ReconnectPolicy;
pub use clock::{Clock, ManualClock, SystemClock};
pub use codes::{Disconnect, ErrorCode, Lang, SuggestedAction};
pub use model::{
    Attachment, ChannelInfo, ClientKind, ClientRef, ConnectionId, EventKind, Grant, Notice,
    QuitReport, QuitSummary, SessionEvent, SessionId, SessionInfo, Target,
};
pub use runtime::{
    DEFAULT_IO_WORKERS, DEFAULT_MAX_BLOCKING_THREADS, DEFAULT_QUIT_BUDGET, DuplicateMode,
    OpenRequest, RuntimeConfig, SessionError, SessionHandle, SessionRuntime, TermFactory,
};
pub use state::{SessionState, StateKind};

use termax_term::{TermConfig, TermCore};

/// The platform's default terminal backend: libghostty-vt on Linux,
/// alacritty_terminal elsewhere.
pub fn default_term(config: TermConfig) -> Box<dyn TermCore> {
    #[cfg(target_os = "linux")]
    {
        Box::new(termax_term_ghostty::GhosttyTerm::new(config))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Box::new(termax_term::AlacrittyTerm::new(config))
    }
}
