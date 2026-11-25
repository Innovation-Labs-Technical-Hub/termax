//! Helpers for endpoint tests: a runtime, an endpoint in a temporary
//! directory, an approver, and an opener for the fixture sshd.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use termax_control::protocol::OpenParams;
use termax_control::{
    ApprovalKind, ApprovalRequest, ClientKind, ClientRegistry, ControlClient, Endpoint, Headless,
    Scope, ServerConfig, SessionOpener, Token,
};
use termax_policy::{PolicyConfig, PolicyEngine, RuleSet};
use termax_session::{ClientRef, OpenRequest, RuntimeConfig, SessionRuntime};
use termax_ssh::{ConnectContext, FixedTrust, MemoryVerifier, TrustChoice};
pub use termax_sshd_fixture::*;

/// Upper bound for one wait.
pub const WAIT: Duration = Duration::from_secs(30);

/// The plain-shell fixture sshd, or `None` (after `SKIP:`).
pub fn start_sshd() -> Option<Sshd> {
    Sshd::start_with(SshdOptions {
        max_sessions: None,
        force_command: Some("exec /bin/sh".into()),
    })
}

macro_rules! sshd_or_skip {
    () => {
        match common::start_sshd() {
            Some(s) => s,
            None => return,
        }
    };
}

/// A runtime that does not watch OS network events.
pub fn runtime_with(mut config: RuntimeConfig) -> SessionRuntime {
    config.watch_os_events = false;
    SessionRuntime::new(config).expect("runtime starts")
}

pub fn runtime() -> SessionRuntime {
    runtime_with(RuntimeConfig::default())
}

/// Opens every `session.open` against the fixture sshd, trusting its key.
pub struct FixtureOpener {
    pub sshd_port: u16,
    pub user: String,
    pub key: std::path::PathBuf,
    pub fingerprint: String,
}

impl FixtureOpener {
    pub fn new(sshd: &Sshd) -> Arc<Self> {
        Arc::new(Self {
            sshd_port: sshd.port,
            user: sshd.user.clone(),
            key: sshd.client_key.clone(),
            fingerprint: sshd.host_fingerprint(),
        })
    }

    pub fn request(&self, owner: ClientRef) -> OpenRequest {
        let mut o = termax_ssh::ConnectOptions::new("127.0.0.1", &self.user);
        o.port = self.sshd_port;
        o.connect_timeout = Duration::from_secs(20);
        o.auth = vec![key_auth(&self.key)];
        let v = Arc::new(MemoryVerifier::new());
        v.insert(
            &format!("127.0.0.1:{}", self.sshd_port),
            "ssh-ed25519",
            &self.fingerprint,
        );
        let ctx = ConnectContext {
            verifier: v,
            trust: Arc::new(FixedTrust(TrustChoice::Reject)),
            prompter: Arc::new(termax_ssh::NoPrompter),
        };
        OpenRequest::new(o, ctx, owner)
    }
}

impl SessionOpener for FixtureOpener {
    fn open_request(&self, params: &OpenParams, owner: ClientRef) -> Result<OpenRequest, String> {
        if params.host != "fixture" {
            return Err(format!("no saved host named {:?}", params.host));
        }
        Ok(self.request(owner))
    }
}

/// An opener that refuses everything (tests that open sessions directly).
pub struct NoOpener;

impl SessionOpener for NoOpener {
    fn open_request(&self, _: &OpenParams, _: ClientRef) -> Result<OpenRequest, String> {
        Err("no hosts".into())
    }
}

/// Policy that allows `echo` and `printf` (and asks for everything else).
pub fn echo_policy() -> Arc<PolicyEngine> {
    let rules = RuleSet::from_toml(
        r#"
        [[rule]]
        program = "echo"
        effect = "allow"
        [[rule]]
        program = "printf"
        effect = "allow"
        "#,
    )
    .expect("rules");
    Arc::new(PolicyEngine::new(PolicyConfig {
        rules,
        rate_limit: None,
    }))
}

pub fn config(opener: Arc<dyn SessionOpener>) -> ServerConfig {
    ServerConfig::new(Arc::new(ClientRegistry::in_memory()), opener, echo_policy())
}

/// A headless endpoint in a fresh temporary directory.
pub struct Fixture {
    pub rt: SessionRuntime,
    pub endpoint: Endpoint,
    pub headless: Option<Headless>,
    pub dir: tempfile::TempDir,
}

impl Fixture {
    pub fn start(rt: SessionRuntime, config: ServerConfig) -> Self {
        // Short path: Unix socket paths are limited to ~104 bytes.
        let dir = tempfile::Builder::new()
            .prefix("tmxc")
            .tempdir_in(if cfg!(unix) {
                std::path::PathBuf::from("/tmp")
            } else {
                std::env::temp_dir()
            })
            .expect("tempdir");
        let endpoint = Endpoint::in_dir(dir.path().join("ep"));
        let headless = Headless::start(&endpoint, rt.service(), config).expect("endpoint starts");
        Self {
            rt,
            endpoint,
            headless: Some(headless),
            dir,
        }
    }

    pub fn headless(&self) -> &Headless {
        self.headless.as_ref().expect("running")
    }

    pub async fn client(&self) -> ControlClient {
        ControlClient::connect(&self.endpoint)
            .await
            .expect("client connects")
    }

    /// Approve the next request matching `pred` (scopes as requested), from
    /// the UI side of the approvals hook.
    pub fn approve_next(
        &self,
        mut pred: impl FnMut(&ApprovalRequest) -> bool + Send + 'static,
    ) -> tokio::task::JoinHandle<ApprovalRequest> {
        let approvals = self.headless().control().approvals();
        let mut rx = approvals.subscribe();
        let pending = approvals.pending();
        tokio::spawn(async move {
            let req = match pending.into_iter().find(|r| pred(r)) {
                Some(r) => r,
                None => loop {
                    let r = rx.recv().await.expect("approval request");
                    if pred(&r) {
                        break r;
                    }
                },
            };
            assert!(approvals.approve(req.id));
            req
        })
    }
}

/// Pair a client with `scopes` and have the "user" approve it.
pub async fn paired(fx: &Fixture, kind: ClientKind, scopes: Vec<Scope>) -> (ControlClient, Token) {
    let c = fx.client().await;
    let approver = fx.approve_next(|r| matches!(r.kind, ApprovalKind::Pairing { .. }));
    let res = c.pair("test-client", kind, scopes).await.expect("paired");
    approver.await.expect("approver");
    (c, res.token)
}
