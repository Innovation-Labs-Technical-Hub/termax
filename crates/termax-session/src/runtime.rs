//! The process-wide session runtime and its registry.

use std::collections::BTreeMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use termax_core::Protocol;
use termax_ssh::{ConnectContext, ConnectOptions, PtyRequest};
use termax_term::{Budget, GridSize, TermConfig, TermCore};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::backoff::ReconnectPolicy;
use crate::clock::{Clock, SystemClock};
use crate::codes::Disconnect;
use crate::log::{ConnectionLog, LogEntry, NullLog};
use crate::model::{
    Attachment, ChannelInfo, ClientRef, ConnectionId, EventKind, Grant, QuitReport, QuitSummary,
    SessionEvent, SessionId, SessionInfo, Target,
};
use crate::net::{
    DEFAULT_ATTEMPT_DELAY, DEFAULT_DNS_TIMEOUT, DEFAULT_RECV_BUFFER, Resolver, SystemResolver,
};
use crate::pipeline::{
    self, DEFAULT_IDLE_COMPACTION_QUIET_PERIOD, DEFAULT_PUBLISH_INTERVAL, FrameReader,
    MAX_WINDOW_BYTES, PipelineConfig, SharedTerm, lock_term,
};
use crate::state::{SessionState, check_transition};

/// Builds the terminal core for a new session.
pub type TermFactory = fn(TermConfig) -> Box<dyn TermCore>;

/// Default I/O worker threads.
pub const DEFAULT_IO_WORKERS: usize = 2;
/// Default cap on blocking-pool threads (DNS, key decryption, log flush).
pub const DEFAULT_MAX_BLOCKING_THREADS: usize = 16;
/// Time allowed for closing sessions and flushing logs on quit.
pub const DEFAULT_QUIT_BUDGET: Duration = Duration::from_secs(2);

/// Runtime settings.
#[derive(Clone)]
pub struct RuntimeConfig {
    /// I/O worker threads; fixed, independent of the session count.
    pub io_workers: usize,
    /// Upper bound on blocking-pool threads.
    pub max_blocking_threads: usize,
    /// Thread name prefix.
    pub thread_name: String,
    /// Reconnect behaviour.
    pub reconnect: ReconnectPolicy,
    /// DNS timeout.
    pub dns_timeout: Duration,
    /// Happy Eyeballs Connection Attempt Delay.
    pub attempt_delay: Duration,
    /// Socket receive buffer.
    pub recv_buffer: u32,
    /// Budget per `TermCore::feed`.
    pub budget: Budget,
    /// Longest gap between frames while output flows.
    pub publish_interval: Duration,
    /// Quiet time before a session's terminal history is compacted.
    ///
    /// The default is provisional: [TO CONFIRM · CHỜ XÁC NHẬN] (owner: Tech
    /// lead); see [`DEFAULT_IDLE_COMPACTION_QUIET_PERIOD`].
    pub idle_compaction_quiet_period: Duration,
    /// Scrollback lines per terminal.
    pub scrollback_lines: usize,
    /// Terminal backend.
    pub term_factory: TermFactory,
    /// Name resolver.
    pub resolver: Arc<dyn Resolver>,
    /// Connection log.
    pub log: Arc<dyn ConnectionLog>,
    /// Time source.
    pub clock: Arc<dyn Clock>,
    /// Events buffered per subscriber before it lags.
    pub event_capacity: usize,
    /// Budget for [`SessionRuntime::quit`].
    pub quit_budget: Duration,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            io_workers: DEFAULT_IO_WORKERS,
            max_blocking_threads: DEFAULT_MAX_BLOCKING_THREADS,
            thread_name: "tmx-io".into(),
            reconnect: ReconnectPolicy::default(),
            dns_timeout: DEFAULT_DNS_TIMEOUT,
            attempt_delay: DEFAULT_ATTEMPT_DELAY,
            recv_buffer: DEFAULT_RECV_BUFFER,
            budget: Budget::default(),
            publish_interval: DEFAULT_PUBLISH_INTERVAL,
            idle_compaction_quiet_period: DEFAULT_IDLE_COMPACTION_QUIET_PERIOD,
            scrollback_lines: 10_000,
            term_factory: crate::default_term,
            resolver: Arc::new(SystemResolver),
            log: Arc::new(NullLog),
            clock: Arc::new(SystemClock),
            event_capacity: 1024,
            quit_budget: DEFAULT_QUIT_BUDGET,
        }
    }
}

impl RuntimeConfig {
    /// Check ranges.
    pub fn validate(&self) -> Result<(), SessionError> {
        let bad = |m: &str| Err(SessionError::InvalidConfig(m.into()));
        if self.io_workers == 0 || self.max_blocking_threads == 0 {
            return bad("io_workers and max_blocking_threads must be at least 1");
        }
        if self.dns_timeout.is_zero() || self.attempt_delay.is_zero() {
            return bad("dns_timeout and attempt_delay must be non-zero");
        }
        if self.idle_compaction_quiet_period.is_zero() {
            return bad("idle_compaction_quiet_period must be non-zero");
        }
        if self.budget.max_bytes == 0 || self.event_capacity == 0 {
            return bad("budget.max_bytes and event_capacity must be non-zero");
        }
        self.reconnect
            .validate()
            .map_err(SessionError::InvalidConfig)
    }
}

/// Errors from the runtime API (session failures are states, not errors).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// Settings out of range.
    #[error("invalid runtime configuration: {0}")]
    InvalidConfig(String),
    /// The tokio runtime could not start.
    #[error("cannot start the session runtime: {0}")]
    Runtime(#[from] io::Error),
    /// No such session (closed or never existed).
    #[error("no session {0}")]
    UnknownSession(SessionId),
    /// The operation needs an open session.
    #[error("session is not open")]
    NotOpen,
    /// The operation failed with a stable code.
    #[error("{0}")]
    Failed(Disconnect),
}

/// Everything needed to open an SSH session.
#[derive(Clone)]
pub struct OpenRequest {
    /// Saved host or ad-hoc.
    pub target: Target,
    /// SSH options (host, port, user, auth, jump hosts, window, keepalive).
    pub connect: ConnectOptions,
    /// Trust store, trust decision and prompter.
    pub context: ConnectContext,
    /// Terminal type and initial size.
    pub pty: PtyRequest,
    /// Client opening the session.
    pub owner: ClientRef,
}

impl OpenRequest {
    /// An ad-hoc request with an 80×24 `xterm-256color` terminal.
    pub fn new(connect: ConnectOptions, context: ConnectContext, owner: ClientRef) -> Self {
        Self {
            target: Target::AdHoc,
            connect,
            context,
            pty: PtyRequest {
                term: "xterm-256color".into(),
                cols: 80,
                rows: 24,
            },
            owner,
        }
    }
}

/// How Duplicate connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicateMode {
    /// New channel on the existing connection, falling back to a new
    /// connection when the server refuses it.
    #[default]
    SameConnection,
    /// "Duplicate as new connection".
    NewConnection,
}

/// An authenticated SSH connection carrying one or more sessions.
pub(crate) struct Conn {
    pub id: ConnectionId,
    pub ssh: termax_ssh::Session,
    /// Sessions with a shell on this connection.
    pub channels: AtomicUsize,
}

pub(crate) enum Command {
    Write(Vec<u8>),
    Resize {
        cols: u16,
        rows: u16,
    },
    ForwardLocal {
        bind: SocketAddr,
        host: String,
        port: u16,
        reply: oneshot::Sender<Result<SocketAddr, SessionError>>,
    },
    Close {
        reply: Option<oneshot::Sender<()>>,
    },
}

/// Per-session state shared by the registry, handles and the session task.
pub(crate) struct SessionCore {
    pub id: SessionId,
    pub info: Mutex<SessionInfo>,
    pub state: watch::Sender<SessionState>,
    pub cmd: mpsc::UnboundedSender<Command>,
    pub term: SharedTerm,
    pub frames: Mutex<Option<FrameReader>>,
    pub conn: Mutex<Option<Arc<Conn>>>,
    pub request: Mutex<OpenRequest>,
    pub shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl SessionCore {
    /// Validated transition; publishes `StateChanged` and logs it.
    pub fn transition(&self, to: SessionState) -> Result<(), crate::state::InvalidTransition> {
        let (from, address) = {
            let mut info = lock(&self.info);
            check_transition(&info.state, &to)?;
            let from = std::mem::replace(&mut info.state, to.clone());
            if to == SessionState::Open {
                info.opened_at = Some(SystemTime::now());
            }
            (from, info.address.clone())
        };
        self.state.send_replace(to.clone());
        let message = match &to {
            SessionState::Disconnected(d) => format!("{from} -> {to}: {}", d.detail),
            _ => format!("{from} -> {to}"),
        };
        tracing::info!(session = %self.id, %address, %message, "session state");
        self.shared.config.log.record(&LogEntry {
            at: SystemTime::now(),
            session: self.id,
            address,
            message,
        });
        self.shared
            .emit(self.id, EventKind::StateChanged { from, to });
        Ok(())
    }

    pub fn state(&self) -> SessionState {
        self.state.borrow().clone()
    }

    pub fn update_info(&self, f: impl FnOnce(&mut SessionInfo)) {
        f(&mut lock(&self.info));
    }
}

pub(crate) struct Shared {
    pub config: RuntimeConfig,
    pub handle: tokio::runtime::Handle,
    pub events: broadcast::Sender<SessionEvent>,
    pub sessions: Mutex<BTreeMap<SessionId, Arc<SessionCore>>>,
    pub disconnects: AtomicUsize,
}

impl Shared {
    pub fn emit(&self, session: SessionId, kind: EventKind) {
        // No subscribers is fine.
        let _ = self.events.send(SessionEvent {
            session,
            at: self.config.clock.now(),
            kind,
        });
    }

    pub fn remove(&self, id: SessionId) {
        lock(&self.sessions).remove(&id);
    }
}

/// One multi-threaded tokio runtime and the registry of every session in
/// the process. All network I/O, DNS, crypto and terminal parsing run on its
/// threads; the methods here only enqueue work and never block the caller,
/// except [`SessionRuntime::quit`] and [`SessionRuntime::block_on`].
pub struct SessionRuntime {
    rt: Option<tokio::runtime::Runtime>,
    shared: Arc<Shared>,
}

impl SessionRuntime {
    /// Start the runtime.
    pub fn new(config: RuntimeConfig) -> Result<Self, SessionError> {
        config.validate()?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(config.io_workers)
            .max_blocking_threads(config.max_blocking_threads)
            .thread_name(config.thread_name.clone())
            .enable_all()
            .build()?;
        let (events, _) = broadcast::channel(config.event_capacity);
        let shared = Arc::new(Shared {
            handle: rt.handle().clone(),
            config,
            events,
            sessions: Mutex::default(),
            disconnects: AtomicUsize::new(0),
        });
        Ok(Self {
            rt: Some(rt),
            shared,
        })
    }

    /// The tokio runtime handle (for async callers such as the CLI).
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.shared.handle
    }

    /// Run a future on the runtime and wait for it. Must not be called from
    /// inside an async context.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.shared.handle.block_on(fut)
    }

    /// I/O worker threads (fixed at start).
    pub fn io_workers(&self) -> usize {
        self.shared.handle.metrics().num_workers()
    }

    /// Open a session. Returns at once; progress arrives as events.
    pub fn open(&self, request: OpenRequest) -> SessionHandle {
        self.spawn_session(request, crate::actor::Start::Fresh)
    }

    /// Duplicate a session: same target and live settings (terminal type
    /// and current size included) on a new channel of the same connection,
    /// or on a new connection.
    pub fn duplicate(
        &self,
        source: SessionId,
        mode: DuplicateMode,
    ) -> Result<SessionHandle, SessionError> {
        let requested_at = Instant::now();
        let src = self.core(source)?;
        let request = lock(&src.request).clone();
        let conn = lock(&src.conn)
            .clone()
            .filter(|c| !c.ssh.is_closed() && mode == DuplicateMode::SameConnection);
        let start = crate::actor::Start::Duplicate {
            source,
            conn,
            requested_at,
        };
        Ok(self.spawn_session(request, start))
    }

    fn spawn_session(&self, request: OpenRequest, start: crate::actor::Start) -> SessionHandle {
        let cfg = &self.shared.config;
        let id = SessionId::new();
        let term = (cfg.term_factory)(TermConfig {
            size: GridSize::new(clamp_u16(request.pty.cols), clamp_u16(request.pty.rows)),
            scrollback_lines: cfg.scrollback_lines,
            ..TermConfig::default()
        });
        let pipe = pipeline::spawn(
            &self.shared.handle,
            term,
            PipelineConfig {
                window_bytes: (request.connect.window_size as usize).min(MAX_WINDOW_BYTES),
                budget: cfg.budget,
                publish_interval: cfg.publish_interval,
                idle_quiet_period: cfg.idle_compaction_quiet_period,
            },
            Arc::clone(&cfg.clock),
        );
        let (cmd, cmd_rx) = mpsc::unbounded_channel();
        let o = &request.connect;
        let info = SessionInfo {
            id,
            target: request.target,
            address: format!("{}@{}", o.user, termax_ssh::host_port(&o.host, o.port)),
            protocol: Protocol::Ssh,
            connection_id: None,
            channels: Vec::new(),
            state: SessionState::Idle,
            created_at: SystemTime::now(),
            opened_at: None,
            owner: request.owner.clone(),
            clients: Vec::new(),
        };
        let core = Arc::new(SessionCore {
            id,
            info: Mutex::new(info),
            state: watch::Sender::new(SessionState::Idle),
            cmd,
            term: Arc::clone(&pipe.term),
            frames: Mutex::new(Some(pipe.frames)),
            conn: Mutex::new(None),
            request: Mutex::new(request),
            shared: Arc::clone(&self.shared),
        });
        lock(&self.shared.sessions).insert(id, Arc::clone(&core));
        crate::actor::spawn(
            Arc::clone(&core),
            cmd_rx,
            pipe.input,
            pipe.events,
            pipe.replies,
            start,
        );
        SessionHandle { core }
    }

    fn core(&self, id: SessionId) -> Result<Arc<SessionCore>, SessionError> {
        lock(&self.shared.sessions)
            .get(&id)
            .cloned()
            .ok_or(SessionError::UnknownSession(id))
    }

    /// A handle to a live session.
    pub fn session(&self, id: SessionId) -> Option<SessionHandle> {
        self.core(id).ok().map(|core| SessionHandle { core })
    }

    /// Every live session, oldest first.
    pub fn list(&self) -> Vec<SessionInfo> {
        lock(&self.shared.sessions)
            .values()
            .map(|c| lock(&c.info).clone())
            .collect()
    }

    /// Receive every session event from now on. A subscriber that falls
    /// more than `event_capacity` events behind gets `Lagged` and should
    /// re-read [`SessionRuntime::list`].
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.shared.events.subscribe()
    }

    /// Record a client attached to a session.
    pub fn attach(
        &self,
        id: SessionId,
        client: ClientRef,
        grant: Grant,
    ) -> Result<(), SessionError> {
        self.core(id)?
            .update_info(|i| i.clients.push(Attachment { client, grant }));
        Ok(())
    }

    /// What quitting now would end.
    pub fn quit_summary(&self) -> QuitSummary {
        let sessions = self.list();
        let forwards = sessions
            .iter()
            .flat_map(|s| {
                s.channels
                    .iter()
                    .filter(|c| matches!(c, ChannelInfo::LocalForward { .. }))
                    .map(|c| (s.id, c.clone()))
            })
            .collect();
        QuitSummary { sessions, forwards }
    }

    /// Quit: notify clients, close every session gracefully (EOF, channel
    /// close, DISCONNECT on each connection) and flush the connection log,
    /// all within `quit_budget`. Blocks the caller; call it from a non-async
    /// thread.
    pub fn quit(&self) -> QuitReport {
        let start = Instant::now();
        let budget = self.shared.config.quit_budget;
        let cores: Vec<_> = lock(&self.shared.sessions).values().cloned().collect();
        for c in &cores {
            self.shared.emit(c.id, EventKind::Quitting);
        }
        let before = self.shared.disconnects.load(Ordering::SeqCst);
        let waits: Vec<_> = cores
            .iter()
            .filter_map(|c| {
                let (tx, rx) = oneshot::channel();
                c.cmd
                    .send(Command::Close { reply: Some(tx) })
                    .ok()
                    .map(|()| rx)
            })
            .collect();
        let log = Arc::clone(&self.shared.config.log);
        let (closed, flushed) = self.block_on(async move {
            let deadline = tokio::time::Instant::now() + budget;
            let mut closed = 0;
            for rx in waits {
                if tokio::time::timeout_at(deadline, rx)
                    .await
                    .is_ok_and(|r| r.is_ok())
                {
                    closed += 1;
                }
            }
            let flush = tokio::task::spawn_blocking(move || log.flush());
            let flushed = matches!(
                tokio::time::timeout_at(deadline, flush).await,
                Ok(Ok(Ok(())))
            );
            (closed, flushed)
        });
        QuitReport {
            sessions_closed: closed,
            sessions_abandoned: cores.len() - closed,
            connections_disconnected: self.shared.disconnects.load(Ordering::SeqCst) - before,
            log_flushed: flushed,
            elapsed: start.elapsed(),
        }
    }
}

impl Drop for SessionRuntime {
    fn drop(&mut self) {
        // Break the registry ↔ session reference cycle.
        lock(&self.shared.sessions).clear();
        // Dropping a runtime from async context panics; never wait here.
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

fn clamp_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// A cheap, cloneable handle to one session.
#[derive(Clone)]
pub struct SessionHandle {
    core: Arc<SessionCore>,
}

impl SessionHandle {
    /// Session id.
    pub fn id(&self) -> SessionId {
        self.core.id
    }

    /// Registry record.
    pub fn info(&self) -> SessionInfo {
        lock(&self.core.info).clone()
    }

    /// Current state.
    pub fn state(&self) -> SessionState {
        self.core.state()
    }

    /// Watch the state (latest value only; use events for every transition).
    pub fn watch_state(&self) -> watch::Receiver<SessionState> {
        self.core.state.subscribe()
    }

    /// Send input to the shell (dropped while not open).
    pub fn write(&self, bytes: impl Into<Vec<u8>>) -> Result<(), SessionError> {
        self.send(Command::Write(bytes.into()))
    }

    /// Resize the terminal and tell the server.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), SessionError> {
        if cols == 0 || rows == 0 {
            return Err(SessionError::InvalidConfig(
                "terminal size must be non-zero".into(),
            ));
        }
        self.send(Command::Resize { cols, rows })
    }

    /// Start a local (`-L`) forward that is restored after reconnects.
    /// Returns the bound address.
    pub async fn forward_local(
        &self,
        bind: SocketAddr,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<SocketAddr, SessionError> {
        let (reply, rx) = oneshot::channel();
        self.send(Command::ForwardLocal {
            bind,
            host: remote_host.to_owned(),
            port: remote_port,
            reply,
        })?;
        rx.await
            .map_err(|_| SessionError::UnknownSession(self.core.id))?
    }

    /// Close gracefully (EOF, channel close, DISCONNECT if it was the last
    /// channel on its connection).
    pub fn close(&self) {
        // Already closing or closed if the task is gone.
        let _ = self.core.cmd.send(Command::Close { reply: None });
    }

    /// Run `f` on the terminal core (input encoding, scrolling, selection,
    /// search). The parser holds the same lock for at most one budget.
    pub fn with_term<R>(&self, f: impl FnOnce(&mut dyn TermCore) -> R) -> R {
        f(lock_term(&self.core.term).as_mut())
    }

    /// Take the frame reader (once; the renderer owns it).
    pub fn take_frame_reader(&self) -> Option<FrameReader> {
        lock(&self.core.frames).take()
    }

    fn send(&self, cmd: Command) -> Result<(), SessionError> {
        self.core
            .cmd
            .send(cmd)
            .map_err(|_| SessionError::UnknownSession(self.core.id))
    }
}
