//! Network-change probes, the handshake limit, signals and the raw output
//! tap, against a real OpenSSH sshd (skipped when sshd or ssh-keygen is
//! missing) and the fixture's fault-injecting proxy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

#[macro_use]
mod common;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use common::*;
use termax_session::{
    ErrorCode, EventKind, OpenRequest, RuntimeConfig, SessionId, SessionState, Signal, StateKind,
};

/// A request through `proxy` with keepalives off, so only a probe can
/// notice a dead path.
fn via_proxy(sshd: &Sshd, proxy: &NetProxy) -> OpenRequest {
    let mut req = request(sshd, Some(proxy.port()));
    req.connect.keepalive_interval = None;
    req
}

fn quiet_config() -> RuntimeConfig {
    RuntimeConfig {
        // OS events would add probes the test does not control.
        watch_os_events: false,
        ..RuntimeConfig::default()
    }
}

/// Verifies: FR-CO01-008 acceptance 1
#[test]
fn network_change_probes_every_connection_and_reconnects_dead_ones() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let rt = runtime(quiet_config());
    let service = rt.service();
    let mut rx = rt.subscribe();
    let dead = rt.open(via_proxy(&sshd, &proxy)).id();
    let healthy = rt.open(request(&sshd, None)).id();
    rt.block_on(async {
        wait_all(&rt, &mut rx, &[dead, healthy], StateKind::Open).await;
        // Wi-Fi → Ethernet: the old path silently drops packets.
        proxy.go_down();
        let called = Instant::now();
        let probe = tokio::spawn({
            let service = service.clone();
            async move { service.network_changed().await }
        });
        let stalled = wait_state(&mut rx, dead, StateKind::Stalled).await;
        let stalled_after = called.elapsed();
        assert_eq!(stalled, SessionState::Stalled);
        let SessionState::Disconnected(d) =
            wait_state(&mut rx, dead, StateKind::Disconnected).await
        else {
            unreachable!()
        };
        assert_eq!(d.code, ErrorCode::NetKeepalive);
        let report = probe.await.unwrap();
        assert_eq!(report.connections, 2);
        assert!(
            report.max_send_delay <= Duration::from_secs(1),
            "every connection probed within 1 s (took {:?})",
            report.max_send_delay
        );
        assert_eq!(report.stalled.len(), 1);
        assert!(
            stalled_after >= Duration::from_secs(5) && stalled_after < Duration::from_secs(8),
            "stalled after the 5 s probe timeout, not a TCP timeout ({stalled_after:?})"
        );
        wait_state(&mut rx, dead, StateKind::Reconnecting).await;
        proxy.come_up();
        wait_state(&mut rx, dead, StateKind::Open).await;
        // The healthy connection was never disturbed.
        assert_eq!(rt.session(healthy).unwrap().state(), SessionState::Open);
        title_round_trip(&rt, &mut rx, dead, "after-network-change").await;
    });
}

/// Verifies: FR-CO01-008 acceptance 2
#[test]
fn resume_with_twenty_sessions_runs_at_most_four_handshakes_at_once() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    // Slow the path a little so handshakes overlap.
    proxy.set_delay(Duration::from_millis(20));
    let rt = runtime(quiet_config());
    let service = rt.service();
    let mut rx = rt.subscribe();
    let ids: Vec<SessionId> = (0..20)
        .map(|_| rt.open(via_proxy(&sshd, &proxy)).id())
        .collect();
    rt.block_on(async {
        wait_all(&rt, &mut rx, &ids, StateKind::Open).await;
        proxy.go_down();
        let report = service.network_changed().await;
        assert_eq!(report.connections, 20);
        assert_eq!(report.stalled.len(), 20);
        proxy.come_up();

        let mut in_flight: HashSet<SessionId> = HashSet::new();
        let mut open: HashSet<SessionId> = HashSet::new();
        let mut peak = 0;
        while open.len() < ids.len() {
            let ev = wait_for(&mut rx, |e| {
                matches!(e.kind, EventKind::StateChanged { .. })
            })
            .await;
            let Some(to) = entered(&ev, ev.session) else {
                continue;
            };
            match to.kind() {
                StateKind::Connecting => {
                    in_flight.insert(ev.session);
                }
                StateKind::Handshaking | StateKind::Authenticating => {}
                StateKind::Open => {
                    in_flight.remove(&ev.session);
                    open.insert(ev.session);
                }
                _ => {
                    in_flight.remove(&ev.session);
                }
            }
            peak = peak.max(in_flight.len());
            assert!(peak <= 4, "{peak} handshakes in parallel");
        }
        assert!(peak >= 2, "handshakes did run in parallel (peak {peak})");
    });
}

#[test]
fn signals_reach_the_remote_process_and_the_output_tap_sees_raw_bytes() {
    // sshd refuses signals for forced commands, so run the login shell.
    let Some(sshd) = Sshd::start_with(SshdOptions {
        max_sessions: None,
        force_command: None,
    }) else {
        return;
    };
    let rt = runtime(quiet_config());
    let mut rx = rt.subscribe();
    let h = rt.open(request(&sshd, None));
    let id = h.id();
    rt.block_on(async {
        wait_state(&mut rx, id, StateKind::Open).await;
        let mut tap = h.subscribe_output();
        h.write("echo tap-$((40+2)); exec sleep 30\n").unwrap();
        let mut seen = Vec::new();
        tokio::time::timeout(WAIT, async {
            while !String::from_utf8_lossy(&seen).contains("tap-42") {
                seen.extend_from_slice(&tap.recv().await.unwrap());
            }
        })
        .await
        .expect("marker through the output tap");
        // `exec` replaced the shell; give it a moment to start sleeping.
        tokio::time::sleep(Duration::from_millis(300)).await;
        h.signal(Signal::Kill).await.unwrap();
        let ev = wait_for(&mut rx, |e| {
            e.session == id && matches!(e.kind, EventKind::Exit(_))
        })
        .await;
        let EventKind::Exit(Some(termax_ssh::ExitStatus::Signal { name, .. })) = ev.kind else {
            panic!("expected an exit by signal, got {:?}", ev.kind);
        };
        assert_eq!(name, "KILL");
    });
}
