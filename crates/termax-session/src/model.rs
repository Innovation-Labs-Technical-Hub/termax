//! Registry records and events.

use std::fmt;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime};

use termax_core::{HostId, Protocol};
use termax_ssh::ExitStatus;
use uuid::Uuid;

use crate::codes::Disconnect;
use crate::state::SessionState;

macro_rules! v7_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            /// A new time-ordered id (UUIDv7).
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

v7_id!(
    /// Stable session id; sorts by creation time.
    SessionId
);
v7_id!(
    /// One SSH transport; shared by sessions duplicated onto it.
    ConnectionId
);

/// What a session connects to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A saved host.
    Host(HostId),
    /// An ad-hoc `user@host:port`.
    AdHoc,
}

/// Kind of client that owns or observes a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    /// The desktop or mobile UI.
    Ui,
    /// `tmx` attached through the control endpoint.
    Cli,
    /// An MCP client.
    Mcp,
    /// Headless mode.
    Headless,
}

/// A client of the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientRef {
    /// Kind.
    pub kind: ClientKind,
    /// Client-chosen identifier (window id, CLI pid, MCP client name).
    pub id: String,
}

impl ClientRef {
    /// A client reference.
    pub fn new(kind: ClientKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

/// What an attached client may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// Read output and state.
    Observe,
    /// Also send input.
    Operate,
}

/// A client attached to a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The client.
    pub client: ClientRef,
    /// Its grant.
    pub grant: Grant,
}

/// A channel a session holds on its connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelInfo {
    /// The interactive shell.
    Shell,
    /// A local (`-L`) forward.
    LocalForward {
        /// Listening address.
        bind: SocketAddr,
        /// Destination host as seen from the server.
        remote_host: String,
        /// Destination port.
        remote_port: u16,
    },
}

/// A registry entry as returned by `SessionRuntime::list`.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    /// Session id.
    pub id: SessionId,
    /// Saved host or ad-hoc.
    pub target: Target,
    /// `user@host:port` of the final hop.
    pub address: String,
    /// Protocol (SSH in this runtime).
    pub protocol: Protocol,
    /// The connection carrying the shell, while connected.
    pub connection_id: Option<ConnectionId>,
    /// Channels held on that connection.
    pub channels: Vec<ChannelInfo>,
    /// Lifecycle state.
    pub state: SessionState,
    /// When the session was created.
    pub created_at: SystemTime,
    /// When the shell last opened.
    pub opened_at: Option<SystemTime>,
    /// Client that opened the session.
    pub owner: ClientRef,
    /// Other clients attached to it.
    pub clients: Vec<Attachment>,
}

/// Something that happened to a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEvent {
    /// Which session.
    pub session: SessionId,
    /// When (runtime clock).
    pub at: Instant,
    /// What.
    pub kind: EventKind,
}

/// Event payloads.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EventKind {
    /// Lifecycle transition.
    StateChanged {
        /// Previous state.
        from: SessionState,
        /// New state.
        to: SessionState,
    },
    /// Window title set by the remote side.
    Title(String),
    /// Working directory reported by the shell (OSC 7).
    Cwd {
        /// Host part, if any.
        host: Option<String>,
        /// Path.
        path: String,
    },
    /// Bell.
    Bell,
    /// The remote shell ended.
    Exit(Option<ExitStatus>),
    /// The session is back after `attempt` reconnect attempts ("Reconnected").
    Reconnected {
        /// Attempt that succeeded.
        attempt: u32,
    },
    /// This session was created by Duplicate.
    Duplicated {
        /// The session it was duplicated from.
        source: SessionId,
        /// Whether it shares the source's connection.
        reused_connection: bool,
        /// Time spent in Termax code, excluding waiting on the network
        /// (only measured when the connection was reused).
        local_overhead: Option<Duration>,
    },
    /// Any other terminal event (clipboard requests, prompt marks, cursor
    /// style), for the UI to handle.
    Terminal(termax_term::TermEvent),
    /// Something the UI should explain (a toast, never a modal).
    Notice(Notice),
    /// Terminal history was compacted after a quiet period.
    Compacted,
    /// The application is quitting; clients should detach.
    Quitting,
}

/// Explanatory notices.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Notice {
    /// Duplicate could not open a channel on the existing connection (for
    /// example the server's session limit), so it opened a new connection.
    DuplicateUsedNewConnection {
        /// The server's refusal.
        reason: String,
    },
    /// An auto-start forward could not be restored after reconnecting.
    ForwardNotRestored {
        /// Its listening address.
        bind: SocketAddr,
        /// Why.
        error: Disconnect,
    },
}

/// What quitting would end; the UI lists it in the confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct QuitSummary {
    /// Live sessions.
    pub sessions: Vec<SessionInfo>,
    /// Live forwards: (session, forward).
    pub forwards: Vec<(SessionId, ChannelInfo)>,
}

/// What quitting did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitReport {
    /// Sessions closed gracefully within the budget.
    pub sessions_closed: usize,
    /// Sessions still closing when the budget ran out.
    pub sessions_abandoned: usize,
    /// SSH connections that were sent a DISCONNECT.
    pub connections_disconnected: usize,
    /// Whether the connection log flushed within the budget.
    pub log_flushed: bool,
    /// Wall time from the call to the return.
    pub elapsed: Duration,
}
