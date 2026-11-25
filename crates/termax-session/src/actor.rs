//! The task that drives one session: connect, serve, reconnect, close.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use termax_ssh::{
    BoxFuture, ConnectContext, Error as SshError, ForwardHandle, HostKeyInfo, HostKeyStatus,
    HostKeyVerifier, ShellChannel, ShellWriter, TrustChoice, TrustDecision,
};
use termax_term::{GridSize, TermEvent};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::codes::{self, Disconnect, ErrorCode};
use crate::model::{ChannelInfo, ConnectionId, EventKind, Notice, SessionId};
use crate::net::Establish;
use crate::pipeline::{MAX_WINDOW_BYTES, PipelineEvent, PipelineInput, lock_term};
use crate::runtime::{Command, Conn, SessionCore, SessionError, Shared};
use crate::state::{SessionState, StateKind};

/// How a session task begins.
pub(crate) enum Start {
    /// Connect from scratch.
    Fresh,
    /// Duplicate `source`, on `conn` when given.
    Duplicate {
        source: SessionId,
        conn: Option<Arc<Conn>>,
        requested_at: Instant,
    },
}

type CurrentWriter = Arc<Mutex<Option<ShellWriter>>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// After a channel ends without an exit status, how long to wait for the
/// connection to report that it closed before treating it as a normal end.
const CLOSE_GRACE: Duration = Duration::from_millis(500);

pub(crate) fn spawn(
    core: Arc<SessionCore>,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    input: PipelineInput,
    events: mpsc::UnboundedReceiver<PipelineEvent>,
    replies: mpsc::UnboundedReceiver<Vec<u8>>,
    start: Start,
) {
    let writer: CurrentWriter = Arc::default();
    let rt = core.shared.handle.clone();
    rt.spawn(forward_events(Arc::clone(&core.shared), core.id, events));
    rt.spawn(forward_replies(Arc::clone(&writer), replies));
    let actor = Actor {
        core,
        cmd_rx,
        input,
        writer,
        reader: None,
        conn: None,
        forwards: Vec::new(),
        attempt: 0,
        was_open: false,
        close_reply: None,
    };
    rt.spawn(actor.run(start));
}

/// Terminal events → session events.
async fn forward_events(
    shared: Arc<Shared>,
    id: SessionId,
    mut rx: mpsc::UnboundedReceiver<PipelineEvent>,
) {
    while let Some(ev) = rx.recv().await {
        let kind = match ev {
            PipelineEvent::Compacted => EventKind::Compacted,
            PipelineEvent::Term { event, .. } => match event {
                TermEvent::Title(t) => EventKind::Title(t),
                TermEvent::ResetTitle => EventKind::Title(String::new()),
                TermEvent::Bell => EventKind::Bell,
                TermEvent::Cwd { host, path } => EventKind::Cwd { host, path },
                other => EventKind::Terminal(other),
            },
        };
        shared.emit(id, kind);
    }
}

/// Terminal replies (DA, DSR, …) → the current shell.
async fn forward_replies(writer: CurrentWriter, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        let w = lock(&writer).clone();
        if let Some(w) = w
            && let Err(e) = w.write(&bytes).await
        {
            tracing::debug!(error = %e, "terminal reply not delivered");
        }
    }
}

struct Forward {
    bind: SocketAddr,
    host: String,
    port: u16,
    handle: Option<ForwardHandle>,
}

enum Phase {
    Connect,
    Serve,
    /// Disconnected for good; wait for Close.
    Wait,
    Close,
}

enum Step {
    Done,
    Failed(Disconnect),
    Closed,
}

struct Actor {
    core: Arc<SessionCore>,
    cmd_rx: mpsc::UnboundedReceiver<Command>,
    input: PipelineInput,
    writer: CurrentWriter,
    reader: Option<JoinHandle<Option<termax_ssh::ExitStatus>>>,
    conn: Option<Arc<Conn>>,
    forwards: Vec<Forward>,
    /// Reconnect attempts since the session was last open.
    attempt: u32,
    /// Auto-reconnect applies only to sessions that were open once.
    was_open: bool,
    close_reply: Option<oneshot::Sender<()>>,
}

impl Actor {
    async fn run(mut self, start: Start) {
        let mut phase = match start {
            Start::Fresh => Phase::Connect,
            Start::Duplicate {
                source,
                conn,
                requested_at,
            } => self.duplicate(source, conn, requested_at).await,
        };
        loop {
            phase = match phase {
                Phase::Connect => match self.connect().await {
                    Step::Done => Phase::Serve,
                    Step::Failed(d) => self.after_failure(d).await,
                    Step::Closed => Phase::Close,
                },
                Phase::Serve => match self.serve().await {
                    Step::Done | Step::Closed => Phase::Close,
                    Step::Failed(d) => self.after_failure(d).await,
                },
                Phase::Wait => {
                    self.interruptible(std::future::pending::<()>()).await;
                    Phase::Close
                }
                Phase::Close => {
                    self.close().await;
                    return;
                }
            }
        }
    }

    fn transition(&self, to: SessionState) {
        if let Err(e) = self.core.transition(to) {
            tracing::warn!(session = %self.core.id, error = %e, "state transition refused");
        }
    }

    fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.core.shared)
    }

    /// Await `fut` while still handling commands. `None` means Close was
    /// requested (or every handle is gone).
    async fn interruptible<T>(&mut self, fut: impl Future<Output = T>) -> Option<T> {
        let mut fut = std::pin::pin!(fut);
        loop {
            tokio::select! {
                r = &mut fut => return Some(r),
                cmd = self.cmd_rx.recv() => match cmd {
                    None => return None,
                    Some(Command::Close { reply }) => {
                        self.close_reply = reply;
                        return None;
                    }
                    Some(other) => self.offline_command(other),
                },
            }
        }
    }

    /// Commands that arrive while there is no shell.
    fn offline_command(&mut self, cmd: Command) {
        match cmd {
            Command::Write(_) => tracing::debug!("input dropped: session not open"),
            Command::Resize { cols, rows } => self.apply_resize(cols, rows),
            Command::ForwardLocal { reply, .. } => {
                let _ = reply.send(Err(SessionError::NotOpen));
            }
            Command::Close { .. } => unreachable!("handled by the caller"),
        }
    }

    /// Resize the local terminal and remember the size for reconnects and
    /// duplicates.
    fn apply_resize(&mut self, cols: u16, rows: u16) {
        // Settings first, so a Duplicate that sees the new terminal size also
        // copies it.
        {
            let mut req = lock(&self.core.request);
            req.pty.cols = u32::from(cols);
            req.pty.rows = u32::from(rows);
        }
        lock_term(&self.core.term).resize(GridSize::new(cols, rows));
    }

    async fn duplicate(
        &mut self,
        source: SessionId,
        conn: Option<Arc<Conn>>,
        requested_at: Instant,
    ) -> Phase {
        if let Some(conn) = conn {
            let pty = lock(&self.core.request).pty.clone();
            let ssh = conn.ssh.clone();
            let before_network = Instant::now();
            match self
                .interruptible(async move { ssh.open_shell(&pty).await })
                .await
            {
                None => return Phase::Close,
                Some(Ok(shell)) => {
                    let after_network = Instant::now();
                    self.attach(conn, shell).await;
                    let overhead = (before_network - requested_at) + after_network.elapsed();
                    self.emit_duplicated(source, true, Some(overhead));
                    return Phase::Serve;
                }
                Some(Err(e @ (SshError::ChannelRefused(_) | SshError::Closed(_)))) => {
                    tracing::info!(session = %self.core.id, error = %e, "duplicate falls back to a new connection");
                    self.core.shared.emit(
                        self.core.id,
                        EventKind::Notice(Notice::DuplicateUsedNewConnection {
                            reason: e.to_string(),
                        }),
                    );
                }
                Some(Err(e)) => return self.after_failure(codes::from_ssh(&e)).await,
            }
        }
        match self.connect().await {
            Step::Done => {
                self.emit_duplicated(source, false, None);
                Phase::Serve
            }
            Step::Failed(d) => self.after_failure(d).await,
            Step::Closed => Phase::Close,
        }
    }

    fn emit_duplicated(&self, source: SessionId, reused: bool, overhead: Option<Duration>) {
        self.core.shared.emit(
            self.core.id,
            EventKind::Duplicated {
                source,
                reused_connection: reused,
                local_overhead: overhead,
            },
        );
    }

    /// Resolve, connect, handshake, authenticate and open the shell.
    async fn connect(&mut self) -> Step {
        let req = lock(&self.core.request).clone();
        if let Err(e) = req.connect.validate() {
            return Step::Failed(codes::from_ssh(&e));
        }
        if req.connect.window_size as usize > MAX_WINDOW_BYTES {
            return Step::Failed(Disconnect::new(
                ErrorCode::ConfigInvalid,
                format!(
                    "window_size {} above {MAX_WINDOW_BYTES}",
                    req.connect.window_size
                ),
            ));
        }
        self.transition(SessionState::Resolving);
        let (host, port) = req
            .connect
            .jump_hosts
            .first()
            .map_or((req.connect.host.clone(), req.connect.port), |h| {
                (h.host.clone(), h.port)
            });
        let shared = self.shared();
        let cfg = &shared.config;
        let est = Establish {
            resolver: cfg.resolver.as_ref(),
            clock: &cfg.clock,
            host: &host,
            port,
            dns_timeout: cfg.dns_timeout,
            connect_timeout: req.connect.connect_timeout,
            attempt_delay: cfg.attempt_delay,
            recv_buffer: cfg.recv_buffer,
        };
        let addrs = match self.interruptible(est.resolve()).await {
            None => return Step::Closed,
            Some(Err(d)) => return Step::Failed(d),
            Some(Ok(a)) => a,
        };
        self.transition(SessionState::Connecting);
        let tcp = match self.interruptible(est.connect(&addrs)).await {
            None => return Step::Closed,
            Some(Err(d)) => return Step::Failed(d),
            Some(Ok(t)) => t,
        };
        self.transition(SessionState::Handshaking);
        let ctx = staged_context(&self.core, &req);
        let ssh = match self
            .interruptible(termax_ssh::Session::connect_stream(&req.connect, &ctx, tcp))
            .await
        {
            None => return Step::Closed,
            Some(Err(e)) => return Step::Failed(codes::from_ssh(&e)),
            Some(Ok(s)) => s,
        };
        to_authenticating(&self.core);
        let conn = Arc::new(Conn {
            id: ConnectionId::new(),
            ssh,
            channels: Default::default(),
        });
        let ssh = conn.ssh.clone();
        let pty = req.pty.clone();
        match self
            .interruptible(async move { ssh.open_shell(&pty).await })
            .await
        {
            None => {
                disconnect_unused(&self.core.shared, &conn).await;
                Step::Closed
            }
            Some(Err(e)) => {
                disconnect_unused(&self.core.shared, &conn).await;
                Step::Failed(codes::from_ssh(&e))
            }
            Some(Ok(shell)) => {
                self.attach(conn, shell).await;
                Step::Done
            }
        }
    }

    /// Wire a fresh shell into the session and move to `Open`.
    async fn attach(&mut self, conn: Arc<Conn>, shell: ShellChannel) {
        let (mut reader, writer) = shell.split();
        *lock(&self.writer) = Some(writer);
        conn.channels.fetch_add(1, Ordering::SeqCst);
        *lock(&self.core.conn) = Some(Arc::clone(&conn));
        let input = self.input.clone();
        self.reader = Some(self.core.shared.handle.spawn(async move {
            while let Some(bytes) = reader.read().await {
                if input.push(bytes).await.is_err() {
                    break;
                }
            }
            reader.exit_status()
        }));
        self.conn = Some(conn);
        self.restore_forwards().await;
        self.update_channels();
        self.transition(SessionState::Open);
        if self.attempt > 0 {
            self.core.shared.emit(
                self.core.id,
                EventKind::Reconnected {
                    attempt: self.attempt,
                },
            );
        }
        self.attempt = 0;
        self.was_open = true;
    }

    fn update_channels(&self) {
        let conn_id = self.conn.as_ref().map(|c| c.id);
        let mut channels = Vec::new();
        if conn_id.is_some() {
            channels.push(ChannelInfo::Shell);
            channels.extend(
                self.forwards
                    .iter()
                    .filter(|f| f.handle.is_some())
                    .map(|f| ChannelInfo::LocalForward {
                        bind: f.bind,
                        remote_host: f.host.clone(),
                        remote_port: f.port,
                    }),
            );
        }
        self.core.update_info(|i| {
            i.connection_id = conn_id;
            i.channels = channels;
        });
    }

    async fn restore_forwards(&mut self) {
        let Some(conn) = self.conn.clone() else {
            return;
        };
        for f in self.forwards.iter_mut().filter(|f| f.handle.is_none()) {
            match conn.ssh.forward_local(f.bind, &f.host, f.port).await {
                Ok(h) => f.handle = Some(h),
                Err(e) => self.core.shared.emit(
                    self.core.id,
                    EventKind::Notice(Notice::ForwardNotRestored {
                        bind: f.bind,
                        error: codes::from_ssh(&e),
                    }),
                ),
            }
        }
    }

    /// The session is open: handle commands until it closes or fails.
    async fn serve(&mut self) -> Step {
        let (Some(conn), Some(mut reader)) = (self.conn.clone(), self.reader.take()) else {
            return Step::Failed(Disconnect::new(ErrorCode::NetLost, "no connection"));
        };
        let ssh = conn.ssh.clone();
        let mut closed = std::pin::pin!(ssh.closed());
        loop {
            tokio::select! {
                reason = &mut closed => {
                    reader.abort();
                    return Step::Failed(close_disconnect(&reason));
                }
                exit = &mut reader => {
                    let status = exit.ok().flatten();
                    if status.is_some() {
                        self.core.shared.emit(self.core.id, EventKind::Exit(status));
                        return Step::Done;
                    }
                    // Channel gone without an exit status: either the
                    // connection is dying or the server closed the channel.
                    return match tokio::time::timeout(CLOSE_GRACE, &mut closed).await {
                        Ok(reason) => Step::Failed(close_disconnect(&reason)),
                        Err(_) => {
                            self.core.shared.emit(self.core.id, EventKind::Exit(None));
                            Step::Done
                        }
                    };
                }
                cmd = self.cmd_rx.recv() => match cmd {
                    None => {
                        reader.abort();
                        return Step::Closed;
                    }
                    Some(Command::Close { reply }) => {
                        self.close_reply = reply;
                        reader.abort();
                        return Step::Closed;
                    }
                    Some(Command::Write(bytes)) => {
                        let w = lock(&self.writer).clone();
                        if let Some(w) = w
                            && let Err(e) = w.write(&bytes).await
                        {
                            tracing::debug!(error = %e, "input not delivered");
                        }
                    }
                    Some(Command::Resize { cols, rows }) => {
                        self.apply_resize(cols, rows);
                        let w = lock(&self.writer).clone();
                        if let Some(w) = w
                            && let Err(e) = w.resize(u32::from(cols), u32::from(rows)).await
                        {
                            tracing::debug!(error = %e, "window change not delivered");
                        }
                    }
                    Some(Command::ForwardLocal { bind, host, port, reply }) => {
                        let r = match ssh.forward_local(bind, &host, port).await {
                            Ok(h) => {
                                let addr = h.local_addr();
                                self.forwards.push(Forward { bind: addr, host, port, handle: Some(h) });
                                self.update_channels();
                                Ok(addr)
                            }
                            Err(e) => Err(SessionError::Failed(codes::from_ssh(&e))),
                        };
                        let _ = reply.send(r);
                    }
                },
            }
        }
    }

    /// Drop the shell and forwards. With `graceful`, send EOF and close on
    /// the channel and DISCONNECT if it was the connection's last channel.
    async fn teardown(&mut self, graceful: bool) {
        let writer = lock(&self.writer).take();
        if let (true, Some(w)) = (graceful, writer.as_ref()) {
            let _ = w.eof().await;
            let _ = w.close().await;
        }
        drop(writer);
        if let Some(r) = self.reader.take() {
            r.abort();
        }
        for f in &mut self.forwards {
            if let Some(h) = f.handle.take() {
                h.close().await;
            }
        }
        *lock(&self.core.conn) = None;
        if let Some(conn) = self.conn.take() {
            let last = conn.channels.fetch_sub(1, Ordering::SeqCst) == 1;
            if graceful && last {
                disconnect_unused(&self.core.shared, &conn).await;
            }
        }
        self.update_channels();
    }

    /// Decide what follows a failure: backoff then reconnect, or stay
    /// disconnected.
    async fn after_failure(&mut self, d: Disconnect) -> Phase {
        self.teardown(false).await;
        let transient = d.is_transient();
        self.transition(SessionState::Disconnected(d));
        let shared = self.shared();
        let policy = &shared.config.reconnect;
        let next = self.attempt + 1;
        if !(transient && self.was_open && policy.allows(next)) {
            return Phase::Wait;
        }
        self.attempt = next;
        let delay = policy.delay(next, fastrand::f64());
        self.transition(SessionState::Reconnecting {
            attempt: next,
            max_attempts: policy.max_attempts,
            delay,
        });
        match self.interruptible(shared.config.clock.sleep(delay)).await {
            Some(()) => Phase::Connect,
            None => Phase::Close,
        }
    }

    async fn close(&mut self) {
        self.teardown(true).await;
        if self.core.state().kind() != StateKind::Closed {
            self.transition(SessionState::Closed);
        }
        self.core.shared.remove(self.core.id);
        if let Some(r) = self.close_reply.take() {
            let _ = r.send(());
        }
    }
}

fn close_disconnect(reason: &termax_ssh::CloseReason) -> Disconnect {
    let code = codes::code_for_close(reason).unwrap_or(ErrorCode::NetLost);
    Disconnect::new(code, reason.to_string())
}

/// DISCONNECT a connection no session uses any more.
async fn disconnect_unused(shared: &Shared, conn: &Conn) {
    if conn.channels.load(Ordering::SeqCst) == 0 && !conn.ssh.is_closed() {
        match conn.ssh.disconnect().await {
            Ok(()) => {
                shared.disconnects.fetch_add(1, Ordering::SeqCst);
            }
            Err(e) => tracing::debug!(error = %e, "disconnect failed"),
        }
    }
}

fn to_authenticating(core: &SessionCore) {
    if core.state().kind() == StateKind::Handshaking
        && let Err(e) = core.transition(SessionState::Authenticating)
    {
        tracing::warn!(session = %core.id, error = %e, "state transition refused");
    }
}

/// Wrap the trust callbacks so the session moves to `Authenticating` once
/// the final hop's host key is accepted.
fn staged_context(core: &Arc<SessionCore>, req: &crate::runtime::OpenRequest) -> ConnectContext {
    let final_hop = termax_ssh::host_port(&req.connect.host, req.connect.port);
    ConnectContext {
        verifier: Arc::new(StagedVerifier {
            inner: Arc::clone(&req.context.verifier),
            core: Arc::clone(core),
            final_hop: final_hop.clone(),
        }),
        trust: Arc::new(StagedTrust {
            inner: Arc::clone(&req.context.trust),
            core: Arc::clone(core),
            final_hop,
        }),
        prompter: Arc::clone(&req.context.prompter),
    }
}

struct StagedVerifier {
    inner: Arc<dyn HostKeyVerifier>,
    core: Arc<SessionCore>,
    final_hop: String,
}

impl HostKeyVerifier for StagedVerifier {
    fn verify<'a>(
        &'a self,
        key: &'a HostKeyInfo,
    ) -> BoxFuture<'a, termax_ssh::Result<HostKeyStatus>> {
        Box::pin(async move {
            let status = self.inner.verify(key).await;
            if matches!(status, Ok(HostKeyStatus::Trusted)) && key.host_port == self.final_hop {
                to_authenticating(&self.core);
            }
            status
        })
    }

    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, termax_ssh::Result<()>> {
        self.inner.save(key)
    }
}

struct StagedTrust {
    inner: Arc<dyn TrustDecision>,
    core: Arc<SessionCore>,
    final_hop: String,
}

impl TrustDecision for StagedTrust {
    fn unknown_host<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, TrustChoice> {
        Box::pin(async move {
            let choice = self.inner.unknown_host(key).await;
            if choice != TrustChoice::Reject && key.host_port == self.final_hop {
                to_authenticating(&self.core);
            }
            choice
        })
    }
}
