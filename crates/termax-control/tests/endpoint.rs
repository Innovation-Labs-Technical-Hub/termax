//! The control endpoint over its real socket, against the fixture sshd
//! where sessions are needed (skipped without sshd).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

#[macro_use]
mod common;

use std::time::Duration;

use base64::Engine;
use common::*;
use serde_json::{Value, json};
use termax_control::protocol::{ListResult, OpenResult, SubscribeResult};
use termax_control::{
    ApprovalKind, ClientKind, ControlClient, ControlError, CtlCode, Headless, RateLimit, Scope,
};
use termax_session::{ClientKind as SessionClientKind, ClientRef, SessionState};

fn code(r: Result<Value, ControlError>) -> Option<CtlCode> {
    r.expect_err("call should fail").code()
}

/// Read `session.output` notifications until their text contains `needle`.
async fn output_contains(c: &mut ControlClient, needle: &str) {
    let mut seen = String::new();
    tokio::time::timeout(WAIT, async {
        while !seen.contains(needle) {
            let n = c.next_notification().await.expect("stream open");
            if n.method != "session.output" {
                continue;
            }
            if let Some(d) = n.params["data"].as_str() {
                let bytes = base64::engine::general_purpose::STANDARD.decode(d).unwrap();
                seen.push_str(&String::from_utf8_lossy(&bytes));
            } else if let Some(t) = n.params["text"].as_str() {
                seen.push_str(t);
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{needle:?} never arrived; saw {seen:?}"));
}

/// Wait until a session opened directly on the runtime is Open.
fn open_direct(fx: &Fixture, opener: &FixtureOpener) -> termax_session::SessionHandle {
    let h = fx
        .rt
        .open(opener.request(ClientRef::new(SessionClientKind::Ui, "window-1")));
    let mut state = h.watch_state();
    fx.rt.block_on(async {
        tokio::time::timeout(WAIT, state.wait_for(|s| *s == SessionState::Open))
            .await
            .expect("session opens")
            .expect("state");
    });
    h
}

#[cfg(unix)]
#[test]
fn socket_and_directory_are_private_and_a_second_runtime_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(fx.endpoint.dir()), 0o700);
    assert_eq!(mode(&fx.endpoint.socket_path()), 0o600);
    let second = Headless::start(
        &fx.endpoint,
        fx.rt.service(),
        config(std::sync::Arc::new(NoOpener)),
    );
    assert!(matches!(second, Err(ControlError::AlreadyRunning(_))));
    // The first runtime still serves.
    fx.rt.block_on(async {
        let c = fx.client().await;
        assert_eq!(
            code(c.call_value("session.list", json!({})).await),
            Some(CtlCode::Unauthorized)
        );
    });
}

/// Verifies: FR-CO01-006 acceptance 2
#[test]
fn unpaired_client_is_unauthorized_until_pairing_is_approved() {
    let fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    fx.rt.block_on(async {
        let c = fx.client().await;
        let err = c.call_value("session.list", json!({})).await.unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Unauthorized));
        assert!(err.to_string().contains("tmx pair"), "{err}");
        assert_eq!(
            c.hello(None).await.unwrap_err().code(),
            Some(CtlCode::Unauthorized)
        );

        let approver = fx.approve_next(|r| matches!(r.kind, ApprovalKind::Pairing { .. }));
        let paired = c
            .pair("tmx", ClientKind::Cli, vec![Scope::Observe])
            .await
            .unwrap();
        let req = approver.await.unwrap();
        let ApprovalKind::Pairing {
            name, pid, scopes, ..
        } = req.kind
        else {
            unreachable!()
        };
        assert_eq!(name, "tmx");
        assert_eq!(scopes, vec![Scope::Observe]);
        #[cfg(unix)]
        assert_eq!(
            pid,
            Some(std::process::id()),
            "the prompt shows the peer's pid"
        );
        let _ = pid;
        assert_eq!(paired.scopes, vec![Scope::Observe]);
        assert_eq!(paired.token.expose().len(), 64);

        // The same call now succeeds with the observe scope.
        let list: ListResult = c.call("session.list", &json!({})).await.unwrap();
        assert!(list.sessions.is_empty());

        // A new connection authenticates with the token.
        let c2 = fx.client().await;
        let hello = c2.hello(Some(paired.token.clone())).await.unwrap();
        assert_eq!(hello.client_id, paired.client_id);
        assert_eq!(hello.scopes, vec![Scope::Observe]);
        let _: ListResult = c2.call("session.list", &json!({})).await.unwrap();

        // The registry keeps only the token's hash; the audit chain has no token.
        let control = fx.headless().control();
        let rec = &control.clients().list()[0];
        assert_eq!(rec.token_sha256, paired.token.hash());
        let audit = control.audit_log();
        audit.verify().unwrap();
        assert!(!audit.to_jsonl().contains(paired.token.expose()));
        assert!(audit.entries().iter().any(|e| e.command == "control.pair"));
    });
}

#[test]
fn denied_or_unanswered_pairing_issues_no_token() {
    let mut cfg = config(std::sync::Arc::new(NoOpener));
    cfg.approval_timeout = Duration::from_millis(300);
    let fx = Fixture::start(runtime(), cfg);
    fx.rt.block_on(async {
        let c = fx.client().await;
        let approvals = fx.headless().control().approvals();
        let mut created = approvals.subscribe();
        let denier = tokio::spawn(async move {
            let r = created.recv().await.unwrap();
            assert!(approvals.deny(r.id));
        });
        let err = c
            .pair("tmx", ClientKind::Cli, vec![Scope::Operate])
            .await
            .unwrap_err();
        denier.await.unwrap();
        assert_eq!(err.code(), Some(CtlCode::Unauthorized));

        // Nobody answers: denied after the timeout, and withdrawn.
        let err = c
            .pair("tmx", ClientKind::Cli, vec![Scope::Operate])
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Unauthorized));
        assert!(fx.headless().control().approvals().pending().is_empty());
        assert!(fx.headless().control().clients().list().is_empty());
    });
}

/// Verifies: FR-PL04-012 acceptance 1
#[test]
fn revoked_client_gets_an_auth_error_and_guidance_to_pair_again() {
    let fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    fx.rt.block_on(async {
        let (c, token) = paired(&fx, ClientKind::Cli, vec![Scope::Observe]).await;
        let id = fx.headless().control().clients().list()[0].id.clone();
        assert!(fx.headless().control().revoke(&id).unwrap());
        // The live connection loses access at once...
        let err = c.call_value("session.list", json!({})).await.unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Unauthorized));
        assert!(err.to_string().contains("pair again"), "{err}");
        // ...and the token no longer authenticates.
        let c2 = fx.client().await;
        let err = c2.hello(Some(token)).await.unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Unauthorized));
        assert!(err.to_string().contains("revoked"), "{err}");
    });
}

/// Verifies: FR-CO01-006 acceptance 3
#[test]
fn observe_only_send_input_is_refused_with_403_and_queues_an_approval() {
    let sshd = sshd_or_skip!();
    let opener = FixtureOpener::new(&sshd);
    let fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    let h = open_direct(&fx, &opener);
    let sid = h.id().to_string();
    fx.rt.block_on(async {
        let (mut c, _) = paired(&fx, ClientKind::Mcp, vec![Scope::Observe]).await;
        let _: SubscribeResult = c
            .call("session.attach", &json!({ "session": sid }))
            .await
            .unwrap();

        let err = c
            .call_value(
                "session.send_input",
                json!({ "session": sid, "data": "echo approved-$((6*7))\n" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Forbidden));
        let ControlError::Rpc {
            data: Some(data), ..
        } = &err
        else {
            unreachable!()
        };
        let approval_id = data["approval_id"]
            .as_str()
            .expect("approval request created");
        let approvals = fx.headless().control().approvals();
        let pending = approvals.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id.to_string(), approval_id);
        assert!(matches!(
            &pending[0].kind,
            ApprovalKind::Operate { method, .. } if method == "session.send_input"
        ));

        // A command the policy denies outright gets no approval request.
        let err = c
            .call_value(
                "session.send_input",
                json!({ "session": sid, "data": "rm -rf /\n" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Forbidden));
        let ControlError::Rpc {
            data: Some(data), ..
        } = &err
        else {
            unreachable!()
        };
        assert!(data.get("approval_id").is_none());
        assert_eq!(approvals.pending().len(), 1);

        // The user approves once: the input reaches the shell.
        assert!(approvals.approve(pending[0].id));
        output_contains(&mut c, "approved-42").await;
        // Approval was for that one call only.
        let err = c
            .call_value(
                "session.special",
                json!({ "session": sid, "special": "signal", "signal": "INT" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Forbidden));
    });
}

/// Verifies: FR-CO01-002 acceptance 1
#[test]
fn operate_client_opens_lists_attaches_types_and_follows_events_over_the_socket() {
    let sshd = sshd_or_skip!();
    let opener = FixtureOpener::new(&sshd);
    let fx = Fixture::start(runtime(), config(opener.clone()));
    // Two tabs opened by the UI.
    let _tabs = [open_direct(&fx, &opener), open_direct(&fx, &opener)];
    fx.rt.block_on(async {
        let (mut c, _) = paired(&fx, ClientKind::Cli, vec![Scope::Operate]).await;
        let sub: SubscribeResult = c.call("session.subscribe", &json!({})).await.unwrap();

        let unknown = c
            .call_value("session.open", json!({ "host": "nope" }))
            .await;
        assert_eq!(code(unknown), Some(CtlCode::Failed));
        let opened: OpenResult = c
            .call("session.open", &json!({ "host": "fixture" }))
            .await
            .unwrap();
        // State changes stream in until the session is open.
        tokio::time::timeout(WAIT, async {
            loop {
                let n = c.next_notification().await.unwrap();
                if n.method == "session.event"
                    && n.params["subscription"] == sub.subscription
                    && n.params["session"] == opened.session
                    && n.params["event"]["to"] == "open"
                {
                    break;
                }
            }
        })
        .await
        .expect("open event");

        let attached: SubscribeResult = c
            .call(
                "session.attach",
                &json!({ "session": opened.session, "mode": "operate" }),
            )
            .await
            .unwrap();
        assert_ne!(attached.subscription, sub.subscription);

        // 3 sessions with state, host, protocol and connection id, and the
        // CLI recorded on the session it attached to.
        let list: ListResult = c.call("session.list", &json!({})).await.unwrap();
        assert_eq!(list.sessions.len(), 3);
        for s in &list.sessions {
            assert_eq!(s.state, "open");
            assert_eq!(s.protocol, "ssh");
            assert!(s.connection_id.is_some());
            assert!(s.address.ends_with(&format!("@127.0.0.1:{}", sshd.port)));
        }
        let mine = list
            .sessions
            .iter()
            .find(|s| s.id == opened.session)
            .unwrap();
        assert_eq!(mine.owner.kind, "cli");
        assert_eq!(mine.clients.len(), 1);
        assert_eq!(mine.clients[0].kind, "cli");
        assert_eq!(mine.clients[0].grant, "operate");
        assert!(
            list.sessions
                .iter()
                .filter(|s| s.id != opened.session)
                .all(|s| s.clients.is_empty())
        );

        // Allowed by policy: typed directly.
        c.call_value(
            "session.send_input",
            json!({ "session": opened.session, "data": "echo typed-$((40+2))\n" }),
        )
        .await
        .unwrap();
        output_contains(&mut c, "typed-42").await;
        // Not covered by a rule: the policy asks, so it waits for approval.
        let err = c
            .call_value(
                "session.send_input",
                json!({ "session": opened.session, "data": "uname\n" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(CtlCode::Forbidden));

        c.call_value(
            "session.special",
            json!({ "session": opened.session, "special": "signal", "signal": "INT" }),
        )
        .await
        .unwrap();
        let bad = c
            .call_value(
                "session.special",
                json!({ "session": opened.session, "special": "reboot" }),
            )
            .await;
        assert!(matches!(bad, Err(ControlError::Rpc { rpc: -32602, .. })));
        let missing = c
            .call_value(
                "session.attach",
                json!({ "session": "0190f7e2-6f5e-7000-8000-000000000000" }),
            )
            .await;
        assert_eq!(code(missing), Some(CtlCode::NotFound));
    });
    // Dropping the connection removes its attachment.
    fx.rt.block_on(async {
        let deadline = tokio::time::Instant::now() + WAIT;
        while fx.rt.list().iter().any(|s| !s.clients.is_empty()) {
            assert!(tokio::time::Instant::now() < deadline, "attachment removed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}

#[test]
fn mcp_clients_get_sanitised_text_instead_of_raw_bytes() {
    let sshd = sshd_or_skip!();
    let opener = FixtureOpener::new(&sshd);
    let fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    let h = open_direct(&fx, &opener);
    let sid = h.id().to_string();
    fx.rt.block_on(async {
        let (mut c, _) = paired(&fx, ClientKind::Mcp, vec![Scope::Observe]).await;
        let _: SubscribeResult = c
            .call("session.attach", &json!({ "session": sid }))
            .await
            .unwrap();
        h.write("printf '\\033[31mred\\033[0m password=hunter2 done\\n'\n")
            .unwrap();
        let mut text = String::new();
        tokio::time::timeout(WAIT, async {
            while !text.contains("done") {
                let n = c.next_notification().await.unwrap();
                assert!(
                    n.params.get("data").is_none(),
                    "no raw bytes for MCP clients"
                );
                text.push_str(n.params["text"].as_str().unwrap_or_default());
            }
        })
        .await
        .expect("output");
        assert!(
            !text.contains('\u{1b}'),
            "escape sequences removed: {text:?}"
        );
        assert!(!text.contains("hunter2"), "secret redacted: {text:?}");
    });
}

#[test]
fn protocol_errors_rate_limits_and_the_audit_chain() {
    let mut cfg = config(std::sync::Arc::new(NoOpener));
    cfg.rate_limit = RateLimit {
        per_second: 0.001,
        burst: 5.0,
    };
    let fx = Fixture::start(runtime(), cfg);
    fx.rt.block_on(async {
        let c = fx.client().await;
        let r = c.call_value("no.such", json!({})).await;
        assert!(matches!(r, Err(ControlError::Rpc { rpc: -32601, .. })));
        let r = c
            .call_value("control.hello", json!({ "api_version": 99 }))
            .await
            .unwrap_err();
        assert_eq!(r.code(), Some(CtlCode::BadRequest));
        let r = c.call_value("control.pair", json!({ "nope": 1 })).await;
        assert!(matches!(r, Err(ControlError::Rpc { rpc: -32602, .. })));
        let _ = c.call_value("session.list", json!({})).await;
        let _ = c.call_value("session.list", json!({})).await;
        // Burst of 5 used up.
        let r = c.call_value("session.list", json!({})).await.unwrap_err();
        assert_eq!(r.code(), Some(CtlCode::RateLimited));

        let audit = fx.headless().control().audit_log();
        audit.verify().unwrap();
        let commands: Vec<_> = audit.entries().iter().map(|e| e.command.as_str()).collect();
        assert_eq!(
            commands,
            [
                "no.such",
                "control.hello",
                "control.pair",
                "session.list",
                "session.list",
                "session.list"
            ]
        );
        assert!(
            audit
                .entries()
                .iter()
                .all(|e| e.verdict == termax_policy::Effect::Deny)
        );
        assert!(audit.entries()[5].reasons[0].contains("TMX-CTL-429"));
    });
    // Garbage gets a parse error.
    #[cfg(unix)]
    fx.rt.block_on(async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::UnixStream::connect(fx.endpoint.socket_path())
            .await
            .unwrap();
        let body = b"{not json";
        s.write_all(&(body.len() as u32).to_be_bytes())
            .await
            .unwrap();
        s.write_all(body).await.unwrap();
        let mut len = [0u8; 4];
        s.read_exact(&mut len).await.unwrap();
        let mut resp = vec![0u8; u32::from_be_bytes(len) as usize];
        s.read_exact(&mut resp).await.unwrap();
        let v: Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    });
}

#[test]
fn shutdown_removes_the_socket() {
    let mut fx = Fixture::start(runtime(), config(std::sync::Arc::new(NoOpener)));
    let headless = fx.headless.take().unwrap();
    fx.rt.block_on(headless.shutdown());
    #[cfg(unix)]
    assert!(!fx.endpoint.socket_path().exists());
    fx.rt.block_on(async {
        assert!(matches!(
            ControlClient::connect(&fx.endpoint).await,
            Err(ControlError::NotRunning(_))
        ));
    });
    // The lock was released: a new runtime can take over.
    Headless::start(
        &fx.endpoint,
        fx.rt.service(),
        config(std::sync::Arc::new(NoOpener)),
    )
    .unwrap();
}
