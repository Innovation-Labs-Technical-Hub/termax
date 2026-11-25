//! The endpoint server: accepts connections, authenticates clients,
//! enforces scopes and rate limits, audits every call and dispatches to the
//! session runtime.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use termax_core::Env;
use termax_policy::{Actor, AuditLog, Effect, ExternalMode, PolicyEngine, Verdict};
use termax_session::{OpenRequest, SessionInfo, SessionService};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::approvals::{ApprovalKind, Approvals, Decision};
use crate::clients::{AuthFailure, ClientKind, ClientRegistry, Scope, allows};
use crate::error::CtlCode;
use crate::frame::{read_frame, write_frame};
use crate::protocol::{
    API_VERSION, HelloParams, HelloResult, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
    OpenParams, PARSE_ERROR, PairParams, PairResult, Request, Response, RpcError,
};
use crate::transport::{Listener, Peer, Stream};

/// How long a pairing or operate approval waits for the user before it is
/// denied.
pub const DEFAULT_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-connection request rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimit {
    /// Sustained calls per second.
    pub per_second: f64,
    /// Burst size.
    pub burst: f64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            per_second: 20.0,
            burst: 40.0,
        }
    }
}

/// Builds sessions for `session.open` from the runtime's own data (the
/// vault in headless mode): targets, credentials and host-key trust never
/// come from the client.
pub trait SessionOpener: Send + Sync + 'static {
    /// The request for `params`, owned by `owner`; `Err` is a message for
    /// the client (no secrets).
    fn open_request(
        &self,
        params: &OpenParams,
        owner: termax_session::ClientRef,
    ) -> Result<OpenRequest, String>;

    /// Environment and tags of a session's host, for the command policy.
    fn host_context(&self, _info: &SessionInfo) -> (Env, Vec<String>) {
        (Env::None, Vec::new())
    }
}

/// Server settings.
pub struct ServerConfig {
    /// Paired clients.
    pub registry: Arc<ClientRegistry>,
    /// Builds `session.open` requests.
    pub opener: Arc<dyn SessionOpener>,
    /// Command policy for `session.send_input`.
    pub policy: Arc<PolicyEngine>,
    /// Wait for the user's decision on approvals.
    pub approval_timeout: Duration,
    /// Per-connection request rate.
    pub rate_limit: RateLimit,
}

impl ServerConfig {
    /// Defaults for everything but the collaborators.
    pub fn new(
        registry: Arc<ClientRegistry>,
        opener: Arc<dyn SessionOpener>,
        policy: Arc<PolicyEngine>,
    ) -> Self {
        Self {
            registry,
            opener,
            policy,
            approval_timeout: DEFAULT_APPROVAL_TIMEOUT,
            rate_limit: RateLimit::default(),
        }
    }
}

pub(crate) struct Shared {
    pub service: SessionService,
    pub config: ServerConfig,
    pub approvals: Arc<Approvals>,
    audit: Mutex<AuditLog>,
    connected: watch::Sender<usize>,
    next_subscription: AtomicU64,
}

impl Shared {
    pub fn subscription_id(&self) -> u64 {
        self.next_subscription.fetch_add(1, Ordering::Relaxed)
    }

    /// Append one call to the audit chain. Only the method, the session id
    /// and the outcome are recorded; never input, output or tokens.
    pub fn audit(&self, client: Option<&str>, command: &str, verdict: &Verdict) {
        let actor = Actor::External {
            client_id: client.unwrap_or("unpaired").to_owned(),
        };
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or_default();
        lock(&self.audit).append(ms, actor, command, verdict);
    }

    /// Wait for the user's decision, denying after the timeout.
    pub async fn decision(
        &self,
        id: crate::approvals::ApprovalId,
        rx: oneshot::Receiver<Decision>,
    ) -> Decision {
        let clock = self.service.clock();
        tokio::select! {
            d = rx => d.unwrap_or(Decision::Deny),
            () = clock.sleep(self.config.approval_timeout) => {
                self.approvals.withdraw(id);
                Decision::Deny
            }
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A running endpoint. Dropping it (or [`ControlHandle::shutdown`]) stops
/// accepting, closes every connection and removes the socket.
pub struct ControlHandle {
    shared: Arc<Shared>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl ControlHandle {
    /// The approval queue: the UI's hook for pairing and operate requests.
    pub fn approvals(&self) -> Arc<Approvals> {
        Arc::clone(&self.shared.approvals)
    }

    /// Paired clients.
    pub fn clients(&self) -> &ClientRegistry {
        &self.shared.config.registry
    }

    /// Revoke a client: its next call fails with `TMX-CTL-401`.
    pub fn revoke(&self, client_id: &str) -> Result<bool, crate::ControlError> {
        let found = self.shared.config.registry.revoke(client_id)?;
        self.shared
            .config
            .policy
            .set_external_mode(client_id, ExternalMode::ObserveOnly);
        Ok(found)
    }

    /// A snapshot of the audit chain of control calls.
    pub fn audit_log(&self) -> AuditLog {
        lock(&self.shared.audit).clone()
    }

    /// Connected clients right now.
    pub fn connected_clients(&self) -> usize {
        *self.shared.connected.borrow()
    }

    /// Watch the number of connected clients.
    pub fn watch_connected(&self) -> watch::Receiver<usize> {
        self.shared.connected.subscribe()
    }

    /// Stop the endpoint and wait until its socket is removed.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take()
            && let Err(e) = task.await
        {
            tracing::warn!(error = %e, "control endpoint task failed");
        }
    }
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Start serving `listener` on the session runtime.
pub fn start(service: SessionService, listener: Listener, config: ServerConfig) -> ControlHandle {
    let shared = Arc::new(Shared {
        service,
        config,
        approvals: Arc::default(),
        audit: Mutex::default(),
        connected: watch::Sender::new(0),
        next_subscription: AtomicU64::new(1),
    });
    let (stop, stop_rx) = oneshot::channel();
    let task = shared
        .service
        .handle()
        .spawn(accept_loop(Arc::clone(&shared), listener, stop_rx));
    ControlHandle {
        shared,
        stop: Some(stop),
        task: Some(task),
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: Listener, mut stop: oneshot::Receiver<()>) {
    let mut conns = JoinSet::new();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    conns.spawn(serve_connection(Arc::clone(&shared), stream, peer));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "control endpoint accept failed");
                    // Avoid a hot loop on a persistent error.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    }
    conns.abort_all();
    while conns.join_next().await.is_some() {}
    drop(listener);
}

/// An authenticated client on one connection.
#[derive(Debug, Clone)]
pub(crate) struct Authed {
    pub id: String,
    pub name: String,
    pub kind: ClientKind,
    pub scopes: Vec<Scope>,
}

impl Authed {
    pub fn client_ref(&self) -> termax_session::ClientRef {
        let kind = match self.kind {
            ClientKind::Cli => termax_session::ClientKind::Cli,
            ClientKind::Mcp => termax_session::ClientKind::Mcp,
        };
        termax_session::ClientRef::new(kind, self.id.clone())
    }
}

/// Per-connection state.
pub(crate) struct Conn {
    pub auth: Option<Authed>,
    pub peer: Peer,
    pub out: mpsc::Sender<Vec<u8>>,
    pub streams: Vec<JoinHandle<()>>,
    pub attachments: Vec<(termax_session::SessionId, termax_session::ClientRef)>,
    tokens: f64,
    refilled: Instant,
}

impl Conn {
    fn take_token(&mut self, now: Instant, limit: RateLimit) -> bool {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * limit.per_second).min(limit.burst);
        self.refilled = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub(crate) fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    // Serialising our own message types cannot fail.
    serde_json::to_vec(msg).unwrap_or_default()
}

async fn serve_connection(shared: Arc<Shared>, stream: Stream, peer: Peer) {
    shared.connected.send_modify(|n| *n += 1);
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(256);
    let writer = tokio::spawn(async move {
        while let Some(body) = out_rx.recv().await {
            if let Err(e) = write_frame(&mut wr, &body).await {
                tracing::debug!(error = %e, "control client went away");
                break;
            }
        }
    });
    let clock = shared.service.clock();
    let mut conn = Conn {
        auth: None,
        peer,
        out: out.clone(),
        streams: Vec::new(),
        attachments: Vec::new(),
        tokens: shared.config.rate_limit.burst,
        refilled: clock.now(),
    };
    loop {
        let frame = match read_frame(&mut rd).await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(error = %e, "control connection closed on a bad frame");
                break;
            }
        };
        if let Some(resp) = handle_frame(&shared, &mut conn, &frame).await
            && out.send(encode(&resp)).await.is_err()
        {
            break;
        }
    }
    for s in conn.streams.drain(..) {
        s.abort();
    }
    for (session, client) in conn.attachments.drain(..) {
        shared.service.detach(session, &client);
    }
    drop(conn);
    drop(out);
    let _ = writer.await;
    shared.connected.send_modify(|n| *n = n.saturating_sub(1));
}

async fn handle_frame(shared: &Arc<Shared>, conn: &mut Conn, frame: &[u8]) -> Option<Response> {
    let value: Value = match serde_json::from_slice(frame) {
        Ok(v) => v,
        Err(_) => {
            return Some(Response::err(
                Value::Null,
                RpcError::rpc(PARSE_ERROR, "invalid JSON"),
            ));
        }
    };
    if value.is_array() {
        return Some(Response::err(
            Value::Null,
            RpcError::rpc(INVALID_REQUEST, "batches are not supported"),
        ));
    }
    let req = match serde_json::from_value::<Request>(value) {
        Ok(r) if r.jsonrpc == "2.0" => r,
        _ => {
            return Some(Response::err(
                Value::Null,
                RpcError::rpc(INVALID_REQUEST, "not a JSON-RPC 2.0 request"),
            ));
        }
    };
    // Notifications from clients have no meaning here.
    let id = req.id.clone()?;
    let session = req
        .params
        .get("session")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let command = match &session {
        Some(s) => format!("{} {s}", req.method),
        None => req.method.clone(),
    };
    let now = shared.service.clock().now();
    let result = if conn.take_token(now, shared.config.rate_limit) {
        dispatch(shared, conn, &req).await
    } else {
        Err(RpcError::ctl(
            CtlCode::RateLimited,
            "too many requests; slow down",
        ))
    };
    let verdict = match &result {
        Ok(_) => Verdict::new(Effect::Allow, "ok"),
        Err(e) => Verdict::new(
            Effect::Deny,
            e.ctl_code()
                .map_or_else(|| format!("rpc {}", e.code), |c| c.as_str().to_owned()),
        ),
    };
    shared.audit(
        conn.auth.as_ref().map(|a| a.id.as_str()),
        &command,
        &verdict,
    );
    Some(match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, e),
    })
}

pub(crate) fn params<T: DeserializeOwned>(v: &Value) -> Result<T, RpcError> {
    serde_json::from_value(v.clone()).map_err(|e| RpcError::rpc(INVALID_PARAMS, e.to_string()))
}

pub(crate) fn to_value<T: Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::ctl(CtlCode::Failed, e.to_string()))
}

/// The authenticated client, if its registration is still valid.
pub(crate) fn authed(shared: &Shared, conn: &mut Conn) -> Result<Authed, RpcError> {
    let Some(a) = conn.auth.clone() else {
        return Err(RpcError::ctl(
            CtlCode::Unauthorized,
            "unauthorized: this client is not paired; run `tmx pair` and approve it in Termax",
        ));
    };
    if !shared.config.registry.is_active(&a.id) {
        conn.auth = None;
        return Err(RpcError::ctl(
            CtlCode::Unauthorized,
            "unauthorized: this client's token was revoked; pair again with `tmx pair`",
        ));
    }
    Ok(a)
}

/// The authenticated client, if it holds `scope`.
pub(crate) fn require(shared: &Shared, conn: &mut Conn, scope: Scope) -> Result<Authed, RpcError> {
    let a = authed(shared, conn)?;
    if allows(&a.scopes, scope) {
        Ok(a)
    } else {
        Err(RpcError::ctl(
            CtlCode::Forbidden,
            format!("forbidden: this call needs the {scope:?} scope").to_lowercase(),
        ))
    }
}

async fn dispatch(shared: &Arc<Shared>, conn: &mut Conn, req: &Request) -> Result<Value, RpcError> {
    match req.method.as_str() {
        "control.hello" => hello(shared, conn, params(&req.params)?).await,
        "control.pair" => pair(shared, conn, params(&req.params)?).await,
        "session.list" => crate::methods::list(shared, conn),
        "session.open" => crate::methods::open(shared, conn, params(&req.params)?).await,
        "session.attach" => crate::methods::attach(shared, conn, params(&req.params)?),
        "session.subscribe" => crate::methods::subscribe(shared, conn, params(&req.params)?),
        "session.send_input" => {
            crate::methods::send_input(shared, conn, params(&req.params)?).await
        }
        "session.special" => crate::methods::special(shared, conn, params(&req.params)?).await,
        other => Err(RpcError::rpc(
            METHOD_NOT_FOUND,
            format!("unknown method {other}"),
        )),
    }
}

fn check_version(v: u32) -> Result<(), RpcError> {
    if v == API_VERSION {
        Ok(())
    } else {
        Err(RpcError::ctl(
            CtlCode::BadRequest,
            format!("unsupported api_version {v}; this runtime speaks {API_VERSION}"),
        )
        .with("api_version", API_VERSION))
    }
}

fn set_auth(shared: &Shared, conn: &mut Conn, a: Authed) {
    let mode = if a.scopes.contains(&Scope::Operate) {
        ExternalMode::Operate
    } else {
        ExternalMode::ObserveOnly
    };
    shared.config.policy.set_external_mode(&a.id, mode);
    conn.auth = Some(a);
}

async fn hello(shared: &Arc<Shared>, conn: &mut Conn, p: HelloParams) -> Result<Value, RpcError> {
    check_version(p.api_version)?;
    let Some(token) = p.token else {
        return Err(RpcError::ctl(
            CtlCode::Unauthorized,
            "unauthorized: no token; run `tmx pair` and approve it in Termax",
        ));
    };
    let registry = Arc::clone(&shared.config.registry);
    let found = tokio::task::spawn_blocking(move || registry.authenticate(&token))
        .await
        .map_err(|e| RpcError::ctl(CtlCode::Failed, e.to_string()))?;
    let rec = match found {
        Ok(r) => r,
        Err(AuthFailure::Revoked) => {
            return Err(RpcError::ctl(
                CtlCode::Unauthorized,
                "unauthorized: this client's token was revoked; pair again with `tmx pair`",
            ));
        }
        Err(AuthFailure::Unknown) => {
            return Err(RpcError::ctl(
                CtlCode::Unauthorized,
                "unauthorized: unknown token; pair again with `tmx pair`",
            ));
        }
    };
    let result = HelloResult {
        api_version: API_VERSION,
        client_id: rec.id.clone(),
        scopes: rec.scopes.clone(),
    };
    set_auth(
        shared,
        conn,
        Authed {
            id: rec.id,
            name: rec.name,
            kind: rec.kind,
            scopes: rec.scopes,
        },
    );
    to_value(&result)
}

/// Executable of a local process, when the OS tells us.
fn binary_path(pid: Option<u32>) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        pid.and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok())
            .map(|p| p.display().to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

async fn pair(shared: &Arc<Shared>, conn: &mut Conn, p: PairParams) -> Result<Value, RpcError> {
    check_version(p.api_version)?;
    let name = p.name.trim();
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(RpcError::ctl(
            CtlCode::BadRequest,
            "name must be 1-64 printable characters",
        ));
    }
    let mut scopes = p.scopes;
    scopes.sort();
    scopes.dedup();
    if scopes.is_empty() {
        return Err(RpcError::ctl(
            CtlCode::BadRequest,
            "request at least one scope",
        ));
    }
    let pid = conn.peer.pid;
    let (req, rx) = shared.approvals.request(ApprovalKind::Pairing {
        name: name.to_owned(),
        kind: p.kind,
        pid,
        binary_path: binary_path(pid),
        scopes: scopes.clone(),
    });
    tracing::info!(approval = %req.id, client = %name, "pairing request waiting for the user");
    let granted = match shared.decision(req.id, rx).await {
        Decision::Approve(None) => scopes,
        Decision::Approve(Some(mut s)) => {
            s.sort();
            s.dedup();
            s
        }
        Decision::Deny => Vec::new(),
    };
    if granted.is_empty() {
        return Err(RpcError::ctl(
            CtlCode::Unauthorized,
            "unauthorized: pairing was denied or not answered in time",
        ));
    }
    let registry = Arc::clone(&shared.config.registry);
    let (name, kind, path) = (name.to_owned(), p.kind, binary_path(pid));
    let issued = tokio::task::spawn_blocking(move || registry.issue(&name, kind, path, granted))
        .await
        .map_err(|e| RpcError::ctl(CtlCode::Failed, e.to_string()))?
        .map_err(|e| RpcError::ctl(CtlCode::Failed, e.to_string()))?;
    let (rec, token) = issued;
    tracing::info!(client_id = %rec.id, client = %rec.name, "client paired");
    let result = PairResult {
        client_id: rec.id.clone(),
        token,
        scopes: rec.scopes.clone(),
    };
    set_auth(
        shared,
        conn,
        Authed {
            id: rec.id,
            name: rec.name,
            kind: rec.kind,
            scopes: rec.scopes,
        },
    );
    to_value(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_limit_allows_a_burst_then_refills_over_time() {
        let limit = RateLimit {
            per_second: 10.0,
            burst: 3.0,
        };
        let t0 = Instant::now();
        let (out, _rx) = mpsc::channel(1);
        let mut c = Conn {
            auth: None,
            peer: Peer::default(),
            out,
            streams: Vec::new(),
            attachments: Vec::new(),
            tokens: limit.burst,
            refilled: t0,
        };
        assert!((0..3).all(|_| c.take_token(t0, limit)));
        assert!(!c.take_token(t0, limit));
        assert!(c.take_token(t0 + Duration::from_millis(100), limit));
        assert!(!c.take_token(t0 + Duration::from_millis(100), limit));
        // Refill never exceeds the burst.
        let later = t0 + Duration::from_secs(60);
        assert!((0..3).all(|_| c.take_token(later, limit)));
        assert!(!c.take_token(later, limit));
    }
}
