//! Caller-thread responsiveness while a handshake is slow. Kept in its own
//! test binary so other tests' load cannot preempt the measured thread.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers outside #[test] functions; a panic is a test failure"
)]

#[macro_use]
mod common;

use std::time::{Duration, Instant};

use common::*;
use termax_session::{EventKind, RuntimeConfig, SessionState, StateKind};

/// Verifies: FR-CO01-001 acceptance 2
#[test]
fn slow_handshake_never_blocks_the_caller_or_other_tabs() {
    let sshd = sshd_or_skip!();
    let proxy = NetProxy::start(sshd.addr());
    let rt = runtime(RuntimeConfig::default());
    let mut rx = rt.subscribe();
    let fast = rt.open(request(&sshd, None));
    rt.block_on(async {
        wait_state(&mut rx, fast.id(), StateKind::Open).await;
        // The other tab's shell has started and answers.
        title_round_trip(&rt, &mut rx, fast.id(), "fast-tab-ready").await;
    });

    // Every packet of the slow session's handshake is delayed by 150 ms.
    proxy.set_delay(Duration::from_millis(150));
    let mut worst = Duration::ZERO;
    let mut timed = |f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        worst = worst.max(t.elapsed());
    };
    let mut slow = None;
    timed(&mut || slow = Some(rt.open(request(&sshd, Some(proxy.port())))));
    let slow = slow.unwrap();
    let echo_marker = "fast-tab-alive";
    let mut echo_sent = None;
    let mut echo_latency = None;
    let mut probe_rx = rt.subscribe();
    let start = Instant::now();
    // The caller (a UI thread) polls the runtime every millisecond while the
    // slow session handshakes, and types into the other tab.
    while slow.state() != SessionState::Open {
        assert!(start.elapsed() < WAIT, "slow session never opened");
        timed(&mut || drop(rt.list()));
        timed(&mut || drop(slow.state()));
        if echo_sent.is_none() && slow.state().kind() == StateKind::Handshaking {
            timed(&mut || {
                fast.write(format!("printf '\\033]0;%s\\007' {echo_marker}\n"))
                    .unwrap();
            });
            echo_sent = Some(Instant::now());
        }
        while let Ok(ev) = probe_rx.try_recv() {
            if ev.session == fast.id()
                && matches!(&ev.kind, EventKind::Title(t) if t == echo_marker)
            {
                echo_latency = echo_sent.map(|t| t.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        start.elapsed() >= Duration::from_millis(600),
        "handshake was slow"
    );
    assert!(
        worst <= Duration::from_millis(4),
        "caller blocked for {worst:?}"
    );
    let echo = echo_latency.expect("other tab answered during the slow handshake");
    assert!(
        echo < Duration::from_millis(500),
        "other tab echo took {echo:?}"
    );
}
