//! `session.*` methods and their streams.

use std::sync::Arc;

use base64::Engine;
use serde_json::{Value, json};
use termax_policy::{Actor, Effect, EvalRequest, Verdict, sanitize_output};
use termax_session::{
    EventKind, Grant, SessionError, SessionEvent, SessionHandle, SessionId, SessionInfo,
    SessionState, Signal, StateKind, Target,
};
use tokio::sync::broadcast::error::RecvError;

use crate::approvals::{ApprovalKind, Decision};
use crate::clients::{ClientKind, Scope, allows};
use crate::error::CtlCode;
use crate::protocol::{
    AttachMode, AttachParams, ClientSummary, INVALID_PARAMS, ListResult, Notification, OpenParams,
    OpenResult, SendInputParams, SessionSummary, SpecialParams, SubscribeParams, SubscribeResult,
};
use crate::server::{Authed, Conn, Shared, authed, encode, require, to_value};

/// Default break length.
pub const DEFAULT_BREAK_MILLIS: u32 = 500;

fn kind_name(k: StateKind) -> String {
    format!("{k:?}").to_lowercase()
}

fn client_summary(c: &termax_session::ClientRef, grant: Option<Grant>) -> ClientSummary {
    ClientSummary {
        kind: format!("{:?}", c.kind).to_lowercase(),
        id: c.id.clone(),
        grant: match grant {
            Some(Grant::Operate) => "operate",
            Some(Grant::Observe) => "observe",
            None => "owner",
        }
        .into(),
    }
}

fn summary(i: &SessionInfo) -> SessionSummary {
    SessionSummary {
        id: i.id.to_string(),
        host_id: match i.target {
            Target::Host(h) => Some(h.to_string()),
            Target::AdHoc => None,
        },
        address: i.address.clone(),
        protocol: format!("{:?}", i.protocol).to_lowercase(),
        connection_id: i.connection_id.map(|c| c.to_string()),
        state: kind_name(i.state.kind()),
        code: match &i.state {
            SessionState::Disconnected(d) => Some(d.code.as_str().to_owned()),
            _ => None,
        },
        owner: client_summary(&i.owner, None),
        clients: i
            .clients
            .iter()
            .map(|a| client_summary(&a.client, Some(a.grant)))
            .collect(),
    }
}

fn session_id(s: &str) -> Result<SessionId, crate::protocol::RpcError> {
    uuid::Uuid::parse_str(s)
        .map(SessionId)
        .map_err(|_| crate::protocol::RpcError::rpc(INVALID_PARAMS, "session must be a session id"))
}

fn session(shared: &Shared, s: &str) -> Result<SessionHandle, crate::protocol::RpcError> {
    let id = session_id(s)?;
    shared.service.session(id).ok_or_else(|| {
        crate::protocol::RpcError::ctl(CtlCode::NotFound, format!("no session {id}"))
    })
}

type R = Result<Value, crate::protocol::RpcError>;

pub(crate) fn list(shared: &Arc<Shared>, conn: &mut Conn) -> R {
    require(shared, conn, Scope::Observe)?;
    to_value(&ListResult {
        sessions: shared.service.list().iter().map(summary).collect(),
    })
}

pub(crate) async fn open(shared: &Arc<Shared>, conn: &mut Conn, p: OpenParams) -> R {
    let a = require(shared, conn, Scope::Operate)?;
    let opener = Arc::clone(&shared.config.opener);
    let owner = a.client_ref();
    let built = tokio::task::spawn_blocking(move || opener.open_request(&p, owner))
        .await
        .map_err(|e| crate::protocol::RpcError::ctl(CtlCode::Failed, e.to_string()))?;
    let request = built.map_err(|m| crate::protocol::RpcError::ctl(CtlCode::Failed, m))?;
    let h = shared.service.open(request);
    to_value(&OpenResult {
        session: h.id().to_string(),
    })
}

pub(crate) fn attach(shared: &Arc<Shared>, conn: &mut Conn, p: AttachParams) -> R {
    let needed = match p.mode {
        AttachMode::Observe => Scope::Observe,
        AttachMode::Operate => Scope::Operate,
    };
    let a = require(shared, conn, needed)?;
    let h = session(shared, &p.session)?;
    let grant = match p.mode {
        AttachMode::Observe => Grant::Observe,
        AttachMode::Operate => Grant::Operate,
    };
    let client = a.client_ref();
    shared
        .service
        .attach(h.id(), client.clone(), grant)
        .map_err(|e| crate::protocol::RpcError::ctl(CtlCode::NotFound, e.to_string()))?;
    conn.attachments.push((h.id(), client));
    let sub = shared.subscription_id();
    let out = conn.out.clone();
    let sanitize = a.kind == ClientKind::Mcp;
    let mut output = h.subscribe_output();
    let mut state = h.watch_state();
    let sid = h.id().to_string();
    drop(h);
    conn.streams.push(tokio::spawn(async move {
        loop {
            tokio::select! {
                chunk = output.recv() => {
                    let params = match chunk {
                        Ok(bytes) if sanitize => json!({
                            "subscription": sub, "session": sid,
                            "text": sanitize_output(&String::from_utf8_lossy(&bytes)),
                        }),
                        Ok(bytes) => json!({
                            "subscription": sub, "session": sid,
                            "data": base64::engine::general_purpose::STANDARD.encode(&bytes[..]),
                        }),
                        Err(RecvError::Lagged(n)) => json!({
                            "subscription": sub, "session": sid, "lagged": n,
                        }),
                        Err(RecvError::Closed) => break,
                    };
                    // Waiting here applies back-pressure; a subscriber that
                    // falls 256 chunks behind gets a `lagged` notice.
                    if out.send(encode(&Notification::new("session.output", params))).await.is_err() {
                        return;
                    }
                }
                changed = state.changed() => {
                    if changed.is_err() || state.borrow().kind() == StateKind::Closed {
                        break;
                    }
                }
            }
        }
        let _ = out
            .send(encode(&Notification::new(
                "session.end",
                json!({ "subscription": sub, "session": sid }),
            )))
            .await;
    }));
    to_value(&SubscribeResult { subscription: sub })
}

fn event_json(ev: &SessionEvent, sanitize: bool) -> Option<Value> {
    let clean = |s: &str| {
        if sanitize {
            sanitize_output(s)
        } else {
            s.to_owned()
        }
    };
    let body = match &ev.kind {
        EventKind::StateChanged { from, to } => {
            let mut v = json!({
                "type": "state_changed",
                "from": kind_name(from.kind()),
                "to": kind_name(to.kind()),
            });
            if let SessionState::Disconnected(d) = to {
                v["code"] = json!(d.code.as_str());
            }
            if let SessionState::Reconnecting { attempt, .. } = to {
                v["attempt"] = json!(attempt);
            }
            v
        }
        EventKind::Title(t) => json!({ "type": "title", "title": clean(t) }),
        EventKind::Cwd { path, .. } => json!({ "type": "cwd", "path": clean(path) }),
        EventKind::Bell => json!({ "type": "bell" }),
        EventKind::Exit(status) => json!({
            "type": "exit",
            "status": match status {
                Some(termax_ssh::ExitStatus::Code(c)) => json!(c),
                Some(termax_ssh::ExitStatus::Signal { name, .. }) => json!(format!("SIG{name}")),
                None => Value::Null,
            },
        }),
        EventKind::Reconnected { attempt } => json!({ "type": "reconnected", "attempt": attempt }),
        EventKind::Quitting => json!({ "type": "quitting" }),
        _ => return None,
    };
    Some(body)
}

pub(crate) fn subscribe(shared: &Arc<Shared>, conn: &mut Conn, p: SubscribeParams) -> R {
    let a = require(shared, conn, Scope::Observe)?;
    let only = p.session.as_deref().map(session_id).transpose()?;
    let sub = shared.subscription_id();
    let out = conn.out.clone();
    let sanitize = a.kind == ClientKind::Mcp;
    let mut events = shared.service.subscribe();
    conn.streams.push(tokio::spawn(async move {
        loop {
            let params = match events.recv().await {
                Ok(ev) if only.is_none_or(|s| s == ev.session) => {
                    let Some(body) = event_json(&ev, sanitize) else {
                        continue;
                    };
                    json!({ "subscription": sub, "session": ev.session.to_string(), "event": body })
                }
                Ok(_) => continue,
                Err(RecvError::Lagged(n)) => json!({ "subscription": sub, "lagged": n }),
                Err(RecvError::Closed) => return,
            };
            if out
                .send(encode(&Notification::new("session.event", params)))
                .await
                .is_err()
            {
                return;
            }
        }
    }));
    to_value(&SubscribeResult { subscription: sub })
}

/// A one-off action that may wait for the user's approval.
enum Action {
    Input(String),
    Signal(Signal),
    Break(u32),
}

impl Action {
    fn summary(&self) -> String {
        match self {
            Action::Input(text) => format!("type {text:?}"),
            Action::Signal(s) => format!("signal {s:?}").to_uppercase(),
            Action::Break(ms) => format!("break {ms} ms"),
        }
    }

    async fn run(self, h: &SessionHandle) -> Result<(), SessionError> {
        match self {
            Action::Input(text) => h.write(text.into_bytes()),
            Action::Signal(s) => h.signal(s).await,
            Action::Break(ms) => h.send_break(ms).await,
        }
    }
}

fn session_failure(e: &SessionError) -> crate::protocol::RpcError {
    let err = crate::protocol::RpcError::ctl(CtlCode::Failed, e.to_string());
    match e {
        SessionError::Failed(d) => err.with("session_code", d.code.as_str()),
        _ => err,
    }
}

/// Queue an approval for `action`; it runs once if the user approves.
/// Returns the `TMX-CTL-403` refusal that names the request.
fn queue_approval(
    shared: &Arc<Shared>,
    a: &Authed,
    method: &str,
    h: &SessionHandle,
    action: Action,
    reason: String,
) -> crate::protocol::RpcError {
    let (req, rx) = shared.approvals.request(ApprovalKind::Operate {
        client_id: a.id.clone(),
        client_name: a.name.clone(),
        method: method.to_owned(),
        session: h.id().to_string(),
        summary: action.summary(),
        reason: reason.clone(),
    });
    let shared2 = Arc::clone(shared);
    let (h, client, command) = (h.clone(), a.id.clone(), format!("{method} {}", h.id()));
    let id = req.id;
    tokio::spawn(async move {
        let verdict = match shared2.decision(id, rx).await {
            Decision::Approve(_) => match action.run(&h).await {
                Ok(()) => Verdict::new(Effect::Allow, format!("approved once ({id})")),
                Err(e) => Verdict::new(Effect::Deny, format!("approved ({id}) but failed: {e}")),
            },
            Decision::Deny => Verdict::new(Effect::Deny, format!("approval {id} denied")),
        };
        shared2.audit(Some(&client), &command, &verdict);
    });
    crate::protocol::RpcError::ctl(
        CtlCode::Forbidden,
        format!("forbidden: {reason}; approval request {id} created"),
    )
    .with("approval_id", id.to_string())
}

pub(crate) async fn send_input(shared: &Arc<Shared>, conn: &mut Conn, p: SendInputParams) -> R {
    let a = authed(shared, conn)?;
    let h = session(shared, &p.session)?;
    let opener = Arc::clone(&shared.config.opener);
    let info = h.info();
    let (env, tags) = tokio::task::spawn_blocking(move || opener.host_context(&info))
        .await
        .map_err(|e| crate::protocol::RpcError::ctl(CtlCode::Failed, e.to_string()))?;
    let actor = Actor::External {
        client_id: a.id.clone(),
    };
    let command = p.data.trim_end_matches(['\r', '\n']);
    let eval = EvalRequest {
        actor: &actor,
        command,
        env,
        tags: &tags,
    };
    let policy = &shared.config.policy;
    if !allows(&a.scopes, Scope::Operate) {
        let rules = policy.rules_verdict(&eval);
        if rules.effect == Effect::Deny {
            return Err(crate::protocol::RpcError::ctl(
                CtlCode::Forbidden,
                format!(
                    "forbidden: observe-only client, and policy denies this input: {}",
                    rules.reasons.join("; ")
                ),
            ));
        }
        return Err(queue_approval(
            shared,
            &a,
            "session.send_input",
            &h,
            Action::Input(p.data),
            "observe-only client".into(),
        ));
    }
    let verdict = policy.evaluate(&eval);
    match verdict.effect {
        Effect::Allow => {
            h.write(p.data.into_bytes())
                .map_err(|e| session_failure(&e))?;
            Ok(json!({}))
        }
        Effect::Ask => Err(queue_approval(
            shared,
            &a,
            "session.send_input",
            &h,
            Action::Input(p.data),
            format!("policy asks: {}", verdict.reasons.join("; ")),
        )),
        Effect::Deny => Err(crate::protocol::RpcError::ctl(
            CtlCode::Forbidden,
            format!("forbidden by policy: {}", verdict.reasons.join("; ")),
        )),
    }
}

/// Parse a signal name (`INT`, `SIGINT`, `int`).
pub fn parse_signal(name: &str) -> Option<Signal> {
    let n = name.trim().to_ascii_uppercase();
    let n = n.strip_prefix("SIG").unwrap_or(&n);
    Some(match n {
        "ABRT" => Signal::Abrt,
        "ALRM" => Signal::Alrm,
        "FPE" => Signal::Fpe,
        "HUP" => Signal::Hup,
        "ILL" => Signal::Ill,
        "INT" => Signal::Int,
        "KILL" => Signal::Kill,
        "PIPE" => Signal::Pipe,
        "QUIT" => Signal::Quit,
        "SEGV" => Signal::Segv,
        "TERM" => Signal::Term,
        "USR1" => Signal::Usr1,
        other
            if !other.is_empty()
                && other.len() <= 16
                && other.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            Signal::Other(other.to_owned())
        }
        _ => return None,
    })
}

pub(crate) async fn special(shared: &Arc<Shared>, conn: &mut Conn, p: SpecialParams) -> R {
    let a = authed(shared, conn)?;
    let h = session(shared, &p.session)?;
    let action = match p.special.as_str() {
        "signal" => {
            let name = p.signal.as_deref().unwrap_or_default();
            Action::Signal(parse_signal(name).ok_or_else(|| {
                crate::protocol::RpcError::rpc(
                    INVALID_PARAMS,
                    "signal must be a signal name such as INT",
                )
            })?)
        }
        "break" => Action::Break(p.millis.unwrap_or(DEFAULT_BREAK_MILLIS).min(10_000)),
        _ => {
            return Err(crate::protocol::RpcError::rpc(
                INVALID_PARAMS,
                "special must be `signal` or `break`",
            ));
        }
    };
    if !allows(&a.scopes, Scope::Operate) {
        return Err(queue_approval(
            shared,
            &a,
            "session.special",
            &h,
            action,
            "observe-only client".into(),
        ));
    }
    action.run(&h).await.map_err(|e| session_failure(&e))?;
    Ok(json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names_parse_with_or_without_sig() {
        assert_eq!(parse_signal("INT"), Some(Signal::Int));
        assert_eq!(parse_signal("sigterm"), Some(Signal::Term));
        assert_eq!(parse_signal("WINCH"), Some(Signal::Other("WINCH".into())));
        assert_eq!(parse_signal(""), None);
        assert_eq!(parse_signal("INT; rm"), None);
    }
}
