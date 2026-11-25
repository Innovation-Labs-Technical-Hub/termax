//! Helpers for the runtime tests against the fixture sshd.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use termax_session::{
    ClientKind, ClientRef, EventKind, OpenRequest, RuntimeConfig, SessionEvent, SessionId,
    SessionRuntime, SessionState, StateKind,
};
use termax_ssh::{ConnectContext, FixedTrust, MemoryVerifier, TrustChoice};
use tokio::sync::broadcast;

pub use termax_sshd_fixture::*;

/// Upper bound for one wait.
pub const WAIT: Duration = Duration::from_secs(30);

/// The fixture sshd running a plain `/bin/sh` for every session, so tests do
/// not depend on the developer's login shell. `None` (skip) without sshd.
pub fn start_sshd(max_sessions: Option<u32>) -> Option<Sshd> {
    Sshd::start_with(SshdOptions {
        max_sessions,
        force_command: Some("exec /bin/sh".into()),
    })
}

/// Start the plain-shell sshd or return from the test (after `SKIP:`).
macro_rules! sshd_or_skip {
    () => {
        match common::start_sshd(None) {
            Some(s) => s,
            None => return,
        }
    };
}

pub fn ui() -> ClientRef {
    ClientRef::new(ClientKind::Ui, "window-1")
}

pub fn runtime(config: RuntimeConfig) -> SessionRuntime {
    SessionRuntime::new(config).expect("runtime starts")
}

/// A context trusting the fixture's host key under `host_port`.
pub fn trusted_as(sshd: &Sshd, host_port: &str) -> ConnectContext {
    let v = Arc::new(MemoryVerifier::new());
    v.insert(host_port, "ssh-ed25519", &sshd.host_fingerprint());
    ctx(v, Arc::new(FixedTrust(TrustChoice::Reject)))
}

/// A request for the fixture sshd, optionally through a proxy port.
pub fn request(sshd: &Sshd, via_port: Option<u16>) -> OpenRequest {
    let mut opts = sshd.opts();
    let port = via_port.unwrap_or(sshd.port);
    opts.port = port;
    let ctx = trusted_as(sshd, &format!("127.0.0.1:{port}"));
    OpenRequest::new(opts, ctx, ui())
}

/// Wait for the first event matching `pred`.
pub async fn wait_for(
    rx: &mut broadcast::Receiver<SessionEvent>,
    mut pred: impl FnMut(&SessionEvent) -> bool,
) -> SessionEvent {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => panic!("timed out waiting for an event"),
            Ok(Ok(ev)) if pred(&ev) => return ev,
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(e)) => panic!("event stream ended: {e}"),
        }
    }
}

/// The state a `StateChanged` event moved to, if it is one for `id`.
pub fn entered(ev: &SessionEvent, id: SessionId) -> Option<&SessionState> {
    match &ev.kind {
        EventKind::StateChanged { to, .. } if ev.session == id => Some(to),
        _ => None,
    }
}

/// Wait until session `id` enters a state of `kind`.
pub async fn wait_state(
    rx: &mut broadcast::Receiver<SessionEvent>,
    id: SessionId,
    kind: StateKind,
) -> SessionState {
    // Same as `wait_for`, but a timeout names the wait and lists what this
    // session did, so CI failures say where they stopped.
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => panic!(
                "timed out waiting for session {id:?} to reach {kind:?}; its events: {seen:#?}"
            ),
            Ok(Ok(ev)) => {
                if let Some(s) = entered(&ev, id)
                    && s.kind() == kind
                {
                    return s.clone();
                }
                if ev.session == id {
                    seen.push(ev);
                }
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                eprintln!("wait_state: event stream lagged, {n} events lost");
            }
            Ok(Err(e)) => panic!("event stream ended: {e}"),
        }
    }
}

/// Wait until every session in `ids` is in a state of `kind` (checks the
/// current state first, so events consumed earlier are not needed).
pub async fn wait_all(
    rt: &SessionRuntime,
    rx: &mut broadcast::Receiver<SessionEvent>,
    ids: &[SessionId],
    kind: StateKind,
) {
    let mut pending: std::collections::HashSet<SessionId> = ids
        .iter()
        .copied()
        .filter(|id| rt.session(*id).is_none_or(|h| h.state().kind() != kind))
        .collect();
    while !pending.is_empty() {
        let ev = wait_for(rx, |e| {
            pending.contains(&e.session) && entered(e, e.session).is_some_and(|s| s.kind() == kind)
        })
        .await;
        pending.remove(&ev.session);
    }
}

/// Type a command that sets the window title to `marker`, and wait for it.
pub async fn title_round_trip(
    rt: &SessionRuntime,
    rx: &mut broadcast::Receiver<SessionEvent>,
    id: SessionId,
    marker: &str,
) -> Duration {
    let h = rt.session(id).expect("session");
    let t = Instant::now();
    h.write(format!("printf '\\033]0;%s\\007' {marker}\n"))
        .expect("write");
    wait_for(rx, |e| {
        e.session == id && matches!(&e.kind, EventKind::Title(t) if t == marker)
    })
    .await;
    t.elapsed()
}

/// Read the SSH banner through a forwarded port.
pub fn banner_through(addr: SocketAddr) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut s = std::net::TcpStream::connect(addr)?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buf = [0u8; 4];
    s.read_exact(&mut buf)?;
    Ok(buf.to_vec())
}
