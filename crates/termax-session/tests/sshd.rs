//! Session runtime tests against a real OpenSSH sshd (skipped when sshd or
//! ssh-keygen is missing) and the fixture's fault-injecting proxy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

#[macro_use]
mod common;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use termax_session::clock::BoxFuture;
use termax_session::log::FileLog;
use termax_session::net::Resolver;
use termax_session::{
    ChannelInfo, ClientKind, ClientRef, Clock, DuplicateMode, ErrorCode, EventKind, Grant, Lang,
    ManualClock, Notice, OpenRequest, RuntimeConfig, SessionState, StateKind, SuggestedAction,
};
use termax_ssh::ConnectOptions;

/// Verifies: FR-CO01-003 acceptance 1
#[test]
fn refused_tcp_disconnects_with_a_stable_code_and_actions() {
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let mut opts = ConnectOptions::new("127.0.0.1", "nobody");
    opts.port = free_port();
    let ctx = ctx(
        Arc::new(termax_ssh::MemoryVerifier::new()),
        Arc::new(termax_ssh::FixedTrust(termax_ssh::TrustChoice::Reject)),
    );
    let h = rt.open(OpenRequest::new(opts, ctx, ui()));
    let id = h.id();
    rt.block_on(async {
        let SessionState::Disconnected(d) = wait_state(&mut rx, id, StateKind::Disconnected).await
        else {
            unreachable!()
        };
        assert_eq!(d.code, ErrorCode::NetRefused);
        assert_eq!(d.code.as_str(), "TMX-NET-002");
        for lang in [Lang::En, Lang::Vi] {
            assert!(!d.text(lang).title.is_empty() && !d.text(lang).message.is_empty());
        }
        assert_ne!(d.text(Lang::En).message, d.text(Lang::Vi).message);
        assert!(d.actions().contains(&SuggestedAction::CheckAddress));
        assert!(!d.detail.is_empty());
        // A session that never opened is not retried; it waits for the user.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(h.state().kind(), StateKind::Disconnected);
    });
    h.close();
}

/// Verifies: FR-CO01-002 acceptance 1
#[test]
fn list_reports_sessions_with_state_host_protocol_connection_and_clients() {
    let sshd = sshd_or_skip!();
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let ids: Vec<_> = (0..3).map(|_| rt.open(request(&sshd, None)).id()).collect();
    rt.block_on(wait_all(&rt, &mut rx, &ids, StateKind::Open));
    let cli = ClientRef::new(ClientKind::Cli, "tmx-4242");
    rt.attach(ids[1], cli.clone(), Grant::Operate).unwrap();

    let list = rt.list();
    assert_eq!(list.len(), 3);
    assert_eq!(
        list.iter().map(|s| s.id).collect::<Vec<_>>(),
        ids,
        "oldest first"
    );
    for s in &list {
        assert_eq!(s.state, SessionState::Open);
        assert_eq!(s.address, format!("{}@127.0.0.1:{}", sshd.user, sshd.port));
        assert_eq!(s.protocol, termax_core::Protocol::Ssh);
        assert!(s.connection_id.is_some());
        assert_eq!(s.channels, [ChannelInfo::Shell]);
        assert_eq!(s.owner.kind, ClientKind::Ui);
        assert!(s.opened_at.is_some());
    }
    assert_eq!(list[1].clients.len(), 1);
    assert_eq!(list[1].clients[0].client, cli);
    assert!(list[0].clients.is_empty() && list[2].clients.is_empty());
    let conns: std::collections::HashSet<_> = list.iter().map(|s| s.connection_id).collect();
    assert_eq!(conns.len(), 3, "separate tabs use separate connections");
}

/// Verifies: FR-CO01-002 acceptance 2
#[test]
fn every_subscriber_gets_state_changes_within_50_ms() {
    let sshd = sshd_or_skip!();
    let rt = runtime(RuntimeConfig::default());
    let subscribers: Vec<_> = (0..4)
        .map(|_| {
            let mut rx = rt.subscribe();
            std::thread::spawn(move || {
                let mut lat = Vec::new();
                while let Ok(ev) = rx.blocking_recv() {
                    let got = Instant::now();
                    if let EventKind::StateChanged { to, .. } = &ev.kind {
                        lat.push(got - ev.at);
                        if *to == SessionState::Closed {
                            break;
                        }
                    }
                }
                lat
            })
        })
        .collect();
    let mut rx = rt.subscribe();
    let h = rt.open(request(&sshd, None));
    rt.block_on(wait_state(&mut rx, h.id(), StateKind::Open));
    h.close();
    rt.block_on(wait_state(&mut rx, h.id(), StateKind::Closed));
    for s in subscribers {
        let lat = s.join().unwrap();
        // Resolving, Connecting, Handshaking, Authenticating, Open, Closed.
        assert_eq!(lat.len(), 6, "{lat:?}");
        let worst = lat.iter().max().unwrap();
        assert!(*worst <= Duration::from_millis(50), "{worst:?}");
    }
}

/// Threads of this process whose name starts with `prefix` (Linux).
#[cfg(target_os = "linux")]
fn threads_named(prefix: &str) -> usize {
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|t| std::fs::read_to_string(t.ok()?.path().join("comm")).ok())
        .filter(|n| n.starts_with(prefix))
        .count()
}

/// Verifies: FR-CO01-001 acceptance 1
#[test]
fn fifty_sessions_share_a_fixed_set_of_io_threads() {
    let sshd = sshd_or_skip!();
    let max_blocking = 4;
    let rt = runtime(RuntimeConfig {
        max_blocking_threads: max_blocking,
        thread_name: "tmx-io-50".into(),
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let mut open = |n: usize| {
        let ids: Vec<_> = (0..n).map(|_| rt.open(request(&sshd, None)).id()).collect();
        rt.block_on(wait_all(&rt, &mut rx, &ids, StateKind::Open));
    };
    open(1);
    assert_eq!(rt.io_workers(), 2);
    #[cfg(target_os = "linux")]
    let after_one = threads_named("tmx-io-50");
    // sshd's MaxStartups throttles many concurrent handshakes; open in batches.
    for _ in 0..7 {
        open(7);
    }
    assert_eq!(rt.list().len(), 50);
    assert!(rt.list().iter().all(|s| s.state == SessionState::Open));
    assert_eq!(rt.io_workers(), 2, "I/O workers do not grow with sessions");
    #[cfg(target_os = "linux")]
    {
        let after_fifty = threads_named("tmx-io-50");
        assert!(
            after_fifty <= 2 + max_blocking,
            "{after_fifty} runtime threads for 50 sessions (1 session: {after_one})"
        );
    }
}

/// Verifies: FR-CO01-004 acceptance 1
#[test]
fn reconnects_one_step_after_the_network_returns_and_restores_forwards() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let mut req = request(&sshd, Some(proxy.port()));
    req.connect.keepalive_interval = Some(Duration::from_secs(1));
    req.connect.keepalive_max_missed = 2;
    let h = rt.open(req);
    let id = h.id();
    rt.block_on(async {
        wait_state(&mut rx, id, StateKind::Open).await;
        let bind = h
            .forward_local(
                SocketAddr::from(([127, 0, 0, 1], 0)),
                "127.0.0.1",
                sshd.port,
            )
            .await
            .unwrap();
        assert_eq!(banner_through(bind).unwrap(), b"SSH-");

        proxy.go_down();
        let SessionState::Disconnected(d) = wait_state(&mut rx, id, StateKind::Disconnected).await
        else {
            unreachable!()
        };
        assert_eq!(d.code, ErrorCode::NetKeepalive);
        // Let a few attempts fail while the network is down.
        let pending_delay = loop {
            match wait_state(&mut rx, id, StateKind::Reconnecting).await {
                SessionState::Reconnecting { attempt, delay, .. } if attempt >= 3 => break delay,
                SessionState::Reconnecting { .. } => {}
                _ => unreachable!(),
            }
        };
        let up = Instant::now();
        proxy.come_up();
        let mut failures_after_up = 0;
        let reconnected = wait_for(&mut rx, |e| {
            if e.session == id
                && matches!(
                    &e.kind,
                    EventKind::StateChanged {
                        to: SessionState::Disconnected(_),
                        ..
                    }
                )
            {
                failures_after_up += 1;
            }
            e.session == id && matches!(e.kind, EventKind::Reconnected { .. })
        })
        .await;
        let took = up.elapsed();
        assert_eq!(
            failures_after_up, 0,
            "first attempt after recovery succeeds"
        );
        assert!(
            took <= pending_delay + Duration::from_secs(3),
            "reconnected {took:?} after recovery (pending wait {pending_delay:?})"
        );
        assert!(matches!(
            reconnected.kind,
            EventKind::Reconnected { attempt: 3 }
        ));
        assert_eq!(h.state(), SessionState::Open);
        // The auto-start forward is back on the same port.
        assert_eq!(banner_through(bind).unwrap(), b"SSH-");
        assert!(h.info().channels.contains(&ChannelInfo::LocalForward {
            bind,
            remote_host: "127.0.0.1".into(),
            remote_port: sshd.port,
        }));
        title_round_trip(&rt, &mut rx, id, "after-reconnect").await;
    });
}

/// Verifies: FR-CO01-004 acceptance 2
#[test]
fn changed_host_key_is_never_retried_and_asks_for_review() {
    let mut sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let h = rt.open(request(&sshd, Some(proxy.port())));
    let id = h.id();
    rt.block_on(wait_state(&mut rx, id, StateKind::Open));
    sshd.rotate_host_key();
    proxy.sever();
    rt.block_on(async {
        // The drop itself is transient, so one attempt runs ...
        wait_state(&mut rx, id, StateKind::Reconnecting).await;
        // ... and meets the new key.
        let d = loop {
            if let SessionState::Disconnected(d) =
                wait_state(&mut rx, id, StateKind::Disconnected).await
                && d.code == ErrorCode::HostKeyChanged
            {
                break d;
            }
        };
        assert!(!d.is_transient());
        assert_eq!(d.actions()[0], SuggestedAction::ReviewHostKey);
        // No further attempt.
        let quiet = tokio::time::timeout(
            Duration::from_secs(3),
            wait_state(&mut rx, id, StateKind::Reconnecting),
        )
        .await;
        assert!(quiet.is_err(), "retried after a host-key change");
        assert_eq!(h.state().kind(), StateKind::Disconnected);
    });
    assert_eq!(proxy.accepted(), 2, "exactly one reconnect attempt");
}

/// Verifies: FR-CO01-004 acceptance 3
#[test]
fn reconnect_waits_follow_1_2_4_seconds_with_jitter_and_number_attempts() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let clock = Arc::new(ManualClock::new());
    let rt = runtime(RuntimeConfig {
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let h = rt.open(request(&sshd, Some(proxy.port())));
    let id = h.id();
    rt.block_on(async {
        wait_state(&mut rx, id, StateKind::Open).await;
        proxy.go_down();
        proxy.sever();
        for (attempt, nominal) in [(1u32, 1.0f64), (2, 2.0), (3, 4.0)] {
            let SessionState::Reconnecting {
                attempt: a,
                max_attempts,
                delay,
            } = wait_state(&mut rx, id, StateKind::Reconnecting).await
            else {
                unreachable!()
            };
            assert_eq!(a, attempt, "overlay shows the attempt number");
            assert_eq!(max_attempts, Some(10));
            let secs = delay.as_secs_f64();
            assert!(
                (nominal * 0.8..=nominal * 1.2).contains(&secs),
                "attempt {attempt}: waited {secs}s, expected {nominal}s ±20%"
            );
            // Nothing happens until the wait has elapsed on the clock.
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(h.state().kind(), StateKind::Reconnecting);
            clock.advance(delay);
            let SessionState::Disconnected(d) =
                wait_state(&mut rx, id, StateKind::Disconnected).await
            else {
                unreachable!()
            };
            assert!(d.is_transient(), "{d}");
        }
    });
}

fn title_marker_session(rt: &termax_session::SessionRuntime, id: termax_session::SessionId) {
    let mut rx = rt.subscribe();
    rt.block_on(title_round_trip(rt, &mut rx, id, "dup-alive"));
}

/// Verifies: FR-CO01-005 acceptance 1
#[test]
fn duplicate_reuses_the_connection_without_new_kex() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let a = rt.open(request(&sshd, Some(proxy.port())));
    rt.block_on(wait_state(&mut rx, a.id(), StateKind::Open));
    a.resize(100, 30).unwrap();
    let size_of = |h: &termax_session::SessionHandle| {
        h.with_term(|t| {
            let mut f = termax_term::Frame::default();
            t.snapshot(&mut f);
            (f.size.cols, f.size.rows)
        })
    };
    while size_of(&a) != (100, 30) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(proxy.accepted(), 1);

    let b = rt.duplicate(a.id(), DuplicateMode::SameConnection).unwrap();
    let dup = rt.block_on(async {
        let state = wait_state(&mut rx, b.id(), StateKind::Open).await;
        assert_eq!(state, SessionState::Open);
        wait_for(&mut rx, |e| {
            e.session == b.id() && matches!(e.kind, EventKind::Duplicated { .. })
        })
        .await
    });
    let EventKind::Duplicated {
        source,
        reused_connection,
        local_overhead,
    } = dup.kind
    else {
        unreachable!()
    };
    assert_eq!(source, a.id());
    assert!(reused_connection);
    let overhead = local_overhead.unwrap();
    assert!(
        overhead <= Duration::from_millis(20),
        "local overhead {overhead:?}"
    );
    assert_eq!(b.info().connection_id, a.info().connection_id);
    assert_eq!(
        proxy.accepted(),
        1,
        "no new TCP connection, so no new KEX or auth"
    );
    // Live settings (terminal size) were copied.
    assert_eq!(size_of(&b), (100, 30));
    title_marker_session(&rt, b.id());
}

/// Verifies: FR-CO01-005 acceptance 2
#[test]
fn duplicate_at_max_sessions_opens_a_new_connection_with_a_notice() {
    let Some(sshd) = start_sshd(Some(1)) else {
        return;
    };
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let a = rt.open(request(&sshd, None));
    rt.block_on(wait_state(&mut rx, a.id(), StateKind::Open));
    let b = rt.duplicate(a.id(), DuplicateMode::SameConnection).unwrap();
    rt.block_on(async {
        let notice = wait_for(&mut rx, |e| {
            e.session == b.id() && matches!(e.kind, EventKind::Notice(_))
        })
        .await;
        assert!(matches!(
            notice.kind,
            EventKind::Notice(Notice::DuplicateUsedNewConnection { .. })
        ));
        wait_state(&mut rx, b.id(), StateKind::Open).await;
    });
    assert_ne!(b.info().connection_id, a.info().connection_id);
    title_marker_session(&rt, b.id());
}

/// A resolver that never answers.
struct BlackHole;
impl Resolver for BlackHole {
    fn name(&self) -> String {
        "black-hole test resolver".into()
    }
    fn lookup(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>> {
        Box::pin(std::future::pending())
    }
}

/// Verifies: FR-CO01-009 acceptance 1
#[test]
fn unresponsive_dns_is_cancellable_and_times_out_with_dns_001() {
    let clock = Arc::new(ManualClock::new());
    let rt = runtime(RuntimeConfig {
        resolver: Arc::new(BlackHole),
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let request = || {
        let opts = ConnectOptions::new("unresponsive.example", "nobody");
        let ctx = ctx(
            Arc::new(termax_ssh::MemoryVerifier::new()),
            Arc::new(termax_ssh::FixedTrust(termax_ssh::TrustChoice::Reject)),
        );
        OpenRequest::new(opts, ctx, ui())
    };
    let t = Instant::now();
    let timed_out = rt.open(request());
    let cancelled = rt.open(request());
    assert!(
        t.elapsed() < Duration::from_millis(4),
        "open() returned at once"
    );
    rt.block_on(async {
        wait_state(&mut rx, timed_out.id(), StateKind::Resolving).await;
        wait_state(&mut rx, cancelled.id(), StateKind::Resolving).await;
        // `Resolving` is reported just before each lookup arms its timeout on
        // the clock; advancing the manual clock earlier would move the
        // deadline. Wait until both lookups are waiting on it.
        let armed = tokio::time::Instant::now() + WAIT;
        while clock.sleepers() < 2 {
            assert!(
                tokio::time::Instant::now() < armed,
                "lookups never armed their timeout"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Cancel one lookup.
        let t = Instant::now();
        cancelled.close();
        wait_state(&mut rx, cancelled.id(), StateKind::Closed).await;
        assert!(t.elapsed() < Duration::from_millis(500));
        // The other times out after 5 s on the session clock.
        clock.advance(Duration::from_millis(4_999));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(timed_out.state(), SessionState::Resolving);
        clock.advance(Duration::from_millis(1));
        let SessionState::Disconnected(d) =
            wait_state(&mut rx, timed_out.id(), StateKind::Disconnected).await
        else {
            unreachable!()
        };
        assert_eq!(d.code.as_str(), "TMX-DNS-001");
        assert!(
            d.detail.contains("black-hole test resolver"),
            "{}",
            d.detail
        );
    });
}

/// Resolves one name to an unreachable IPv6 address first, then IPv4.
struct DualStack {
    port: u16,
}
impl Resolver for DualStack {
    fn name(&self) -> String {
        "dual-stack test resolver".into()
    }
    fn lookup(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>> {
        // 100::/64 is the discard-only prefix (RFC 6666): never reachable.
        let v6 = SocketAddr::from(([0x100, 0, 0, 0, 0, 0, 0, 1], self.port));
        let v4 = SocketAddr::from(([127, 0, 0, 1], self.port));
        Box::pin(async move { Ok(vec![v6, v4]) })
    }
}

/// Verifies: FR-CO01-009 acceptance 2
#[test]
fn unreachable_aaaa_falls_back_to_ipv4_within_300_ms() {
    let sshd = sshd_or_skip!();
    let rt = runtime(RuntimeConfig {
        resolver: Arc::new(DualStack { port: sshd.port }),
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let mut opts = sshd.opts();
    opts.host = "dualstack.test".into();
    let host_port = format!("dualstack.test:{}", sshd.port);
    let h = rt.open(OpenRequest::new(opts, trusted_as(&sshd, &host_port), ui()));
    rt.block_on(async {
        let connecting = wait_for(&mut rx, |e| {
            entered(e, h.id()).is_some_and(|s| s.kind() == StateKind::Connecting)
        })
        .await;
        let connected = wait_for(&mut rx, |e| {
            entered(e, h.id()).is_some_and(|s| s.kind() == StateKind::Handshaking)
        })
        .await;
        let took = connected.at - connecting.at;
        assert!(
            took <= Duration::from_millis(300),
            "IPv4 fallback took {took:?}"
        );
        wait_state(&mut rx, h.id(), StateKind::Open).await;
    });
}

/// Verifies: FR-CO01-010 acceptance 1
#[test]
fn quit_lists_sessions_and_forwards_then_disconnects_all_and_flushes() {
    let sshd = sshd_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("connections.log");
    let rt = runtime(RuntimeConfig {
        log: Arc::new(FileLog::open(&log_path).unwrap()),
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let sessions: Vec<_> = (0..3).map(|_| rt.open(request(&sshd, None))).collect();
    rt.block_on(async {
        let ids: Vec<_> = sessions.iter().map(|s| s.id()).collect();
        wait_all(&rt, &mut rx, &ids, StateKind::Open).await;
        for s in &sessions[..2] {
            s.forward_local(
                SocketAddr::from(([127, 0, 0, 1], 0)),
                "127.0.0.1",
                sshd.port,
            )
            .await
            .unwrap();
        }
    });
    let summary = rt.quit_summary();
    assert_eq!(summary.sessions.len(), 3);
    assert_eq!(summary.forwards.len(), 2);

    let report = rt.quit();
    assert_eq!(report.sessions_closed, 3);
    assert_eq!(report.sessions_abandoned, 0);
    assert_eq!(report.connections_disconnected, 3);
    assert!(report.log_flushed);
    assert!(
        report.elapsed <= Duration::from_secs(2),
        "{:?}",
        report.elapsed
    );
    assert!(rt.list().is_empty());

    // The server saw a DISCONNECT on every connection.
    let deadline = Instant::now() + Duration::from_secs(5);
    let count = || {
        sshd.log()
            .matches("Received disconnect from 127.0.0.1")
            .count()
    };
    while count() < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(count(), 3, "{}", sshd.log());
    // The flushed log has every session's Open -> Closed transition.
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(log.matches("Open -> Closed").count(), 3, "{log}");
}

/// Scenario: FR-CO01-011 quiet session is compacted (an SSH session whose
/// terminal holds 10,000 lines is compacted once after the quiet period; the
/// memory bound is measured by `cargo xtask perf`)
#[test]
fn quiet_ssh_session_is_compacted_once() {
    let sshd = sshd_or_skip!();
    let clock = Arc::new(ManualClock::new());
    let rt = runtime(RuntimeConfig {
        clock: Arc::clone(&clock) as Arc<dyn Clock>,
        ..RuntimeConfig::default()
    });
    let mut rx = rt.subscribe();
    let h = rt.open(request(&sshd, None));
    let id = h.id();
    rt.block_on(async {
        wait_state(&mut rx, id, StateKind::Open).await;
        h.write("seq -f '%08g xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx' 1 10000; printf '\\033]0;%s\\007' filled\n")
            .unwrap();
        wait_for(&mut rx, |e| {
            e.session == id && matches!(&e.kind, EventKind::Title(t) if t == "filled")
        })
        .await;
        let lines = h.with_term(|t| {
            let mut f = termax_term::Frame::default();
            t.snapshot(&mut f);
            f.history_len + usize::from(f.size.rows)
        });
        assert!(lines >= 10_000, "terminal holds {lines} lines");
        clock.advance(termax_session::pipeline::DEFAULT_IDLE_COMPACTION_QUIET_PERIOD);
        wait_for(&mut rx, |e| e.session == id && e.kind == EventKind::Compacted).await;
        clock.advance(termax_session::pipeline::DEFAULT_IDLE_COMPACTION_QUIET_PERIOD * 3);
        let again = tokio::time::timeout(
            Duration::from_millis(300),
            wait_for(&mut rx, |e| e.session == id && e.kind == EventKind::Compacted),
        )
        .await;
        assert!(again.is_err(), "compacted twice without new output");
    });
}
