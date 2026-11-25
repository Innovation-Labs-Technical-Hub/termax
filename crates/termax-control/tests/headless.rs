//! Headless mode: idle exit and sessions from the shared vault.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

#[macro_use]
mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use serde_json::json;
use termax_control::protocol::{ListResult, OpenResult};
use termax_control::{ClientKind, DEFAULT_IDLE_TIMEOUT, Scope, VaultOpener};
use termax_core::{Host, Vault, VaultId, VaultKind};
use termax_session::{ManualClock, RuntimeConfig};
use termax_vault::{KdfParams, KnownHost, VaultStore};

async fn until(mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not reached"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Verifies: FR-CO01-007 acceptance 2
#[test]
fn headless_exits_when_idle_and_removes_its_socket() {
    let clock = Arc::new(ManualClock::new());
    let rt = runtime_with(RuntimeConfig {
        clock: clock.clone(),
        ..RuntimeConfig::default()
    });
    let mut fx = Fixture::start(rt, config(Arc::new(NoOpener)));
    assert_eq!(DEFAULT_IDLE_TIMEOUT, Duration::from_secs(300));
    let headless = fx.headless.take().unwrap();
    fx.rt.block_on(async {
        let headless = Arc::new(headless);
        let h = Arc::clone(&headless);
        let idle = tokio::spawn(async move { h.wait_until_idle(DEFAULT_IDLE_TIMEOUT).await });
        until(|| clock.sleepers() > 0).await;
        clock.advance(Duration::from_secs(240));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!idle.is_finished(), "not idle long enough yet");

        // A client connects: the runtime is busy, then idle again once it leaves.
        let c = termax_control::ControlClient::connect(&fx.endpoint)
            .await
            .unwrap();
        until(|| headless.control().connected_clients() == 1).await;
        clock.advance(Duration::from_secs(600));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!idle.is_finished(), "a connected client keeps it alive");
        drop(c);
        until(|| headless.control().connected_clients() == 0).await;
        until(|| clock.sleepers() > 0).await;
        clock.advance(Duration::from_secs(299));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!idle.is_finished(), "the idle period restarted");
        clock.advance(Duration::from_secs(1));
        tokio::time::timeout(WAIT, idle).await.unwrap().unwrap();

        let headless = Arc::into_inner(headless).expect("sole owner");
        headless.shutdown().await;
    });
    #[cfg(unix)]
    assert!(
        !fx.endpoint.socket_path().exists(),
        "socket removed on exit"
    );
}

/// Verifies: FR-CO01-007 acceptance 1
#[test]
fn headless_runtime_opens_a_vault_host_and_lists_it() {
    let sshd = sshd_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let secret = termax_vault::generate_secret_key().unwrap();
    let mut store = VaultStore::create_with_params(
        dir.path().join("vault.db"),
        "password-123",
        &secret,
        KdfParams::new(8, 1, 1).unwrap(),
    )
    .unwrap();
    let vault = Vault {
        id: VaultId::new(),
        name: "Personal".into(),
        kind: VaultKind::Personal,
        settings: Default::default(),
    };
    store.create_vault(&vault).unwrap();
    let mut host = Host::new(vault.id, "prod-db", "127.0.0.1");
    host.port = Some(sshd.port);
    host.settings.set("user", sshd.user.clone());
    host.settings
        .set("identityfile", sshd.client_key.display().to_string());
    store.create_host(&host).unwrap();
    store
        .upsert_known_host(&KnownHost::new(
            sshd.host_port(),
            "ssh-ed25519",
            sshd.host_fingerprint(),
            "test",
        ))
        .unwrap();
    let store = Arc::new(Mutex::new(store));
    let opener = Arc::new(VaultOpener::new(store, vault.id, "nobody".into()));

    let fx = Fixture::start(runtime(), config(opener));
    fx.rt.block_on(async {
        let (c, _) = paired(&fx, ClientKind::Cli, vec![Scope::Operate]).await;
        let opened: OpenResult = c
            .call("session.open", &json!({ "host": "prod-db" }))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + WAIT;
        let listed = loop {
            let list: ListResult = c.call("session.list", &json!({})).await.unwrap();
            let s = list
                .sessions
                .into_iter()
                .find(|s| s.id == opened.session)
                .unwrap();
            if s.state == "open" {
                break s;
            }
            assert!(s.code.is_none(), "session failed: {:?}", s.code);
            assert!(
                tokio::time::Instant::now() < deadline,
                "session never opened"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(listed.host_id, Some(host.id.to_string()));
        assert_eq!(
            listed.address,
            format!("{}@127.0.0.1:{}", sshd.user, sshd.port)
        );

        let missing = c
            .call_value("session.open", json!({ "host": "nope" }))
            .await;
        assert!(missing.unwrap_err().to_string().contains("no saved host"));
    });
}

#[test]
fn unknown_host_keys_are_rejected_in_headless_mode() {
    let sshd = sshd_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let secret = termax_vault::generate_secret_key().unwrap();
    let mut store = VaultStore::create_with_params(
        dir.path().join("vault.db"),
        "password-123",
        &secret,
        KdfParams::new(8, 1, 1).unwrap(),
    )
    .unwrap();
    let vault = Vault {
        id: VaultId::new(),
        name: "Personal".into(),
        kind: VaultKind::Personal,
        settings: Default::default(),
    };
    store.create_vault(&vault).unwrap();
    let mut host = Host::new(vault.id, "new-box", "127.0.0.1");
    host.port = Some(sshd.port);
    host.settings.set("user", sshd.user.clone());
    host.settings
        .set("identityfile", sshd.client_key.display().to_string());
    store.create_host(&host).unwrap();
    let opener = Arc::new(VaultOpener::new(
        Arc::new(Mutex::new(store)),
        vault.id,
        "nobody".into(),
    ));
    let fx = Fixture::start(runtime(), config(opener));
    fx.rt.block_on(async {
        let (c, _) = paired(&fx, ClientKind::Cli, vec![Scope::Operate]).await;
        let opened: OpenResult = c
            .call("session.open", &json!({ "host": "new-box" }))
            .await
            .unwrap();
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let list: ListResult = c.call("session.list", &json!({})).await.unwrap();
            let s = list
                .sessions
                .into_iter()
                .find(|s| s.id == opened.session)
                .unwrap();
            if s.state == "disconnected" {
                assert_eq!(s.code.as_deref(), Some("TMX-HOSTKEY-003"));
                break;
            }
            assert_ne!(s.state, "open", "unknown host key must not be trusted");
            assert!(tokio::time::Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
}
