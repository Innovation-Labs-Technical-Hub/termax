//! OS network-change and resume detection.
//!
//! Both trigger [`SessionService::network_changed`], which probes every
//! connection so dead ones reconnect within seconds instead of waiting for a
//! TCP or keepalive timeout.
//!
//! - Network changes come from `if-watch`: interface address events from
//!   netlink on Linux, SystemConfiguration on macOS and
//!   `NotifyIpInterfaceChange` on Windows. Bursts are debounced
//!   ([`NETWORK_DEBOUNCE`]). If the watcher cannot start (for example
//!   without netlink access in a sandbox) a warning is logged and only
//!   resume detection and keepalives remain.
//! - Resume from sleep has no maintained cross-platform notification crate.
//!   While at least one connection is open, the wall clock is compared with
//!   the monotonic clock every [`RESUME_CHECK_INTERVAL`]; the monotonic
//!   clock stops during sleep on Linux and macOS, so a gap above
//!   [`RESUME_GAP`] means the machine slept. On Windows the monotonic clock
//!   may keep running during sleep, so a resume is noticed through the
//!   network events that usually follow it, or through keepalives.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::mpsc;

use crate::clock::Clock;
use crate::runtime::SessionService;

/// Quiet time after the last network event before probing.
pub const NETWORK_DEBOUNCE: Duration = Duration::from_millis(100);
/// How often the resume check runs while a connection is open.
pub const RESUME_CHECK_INTERVAL: Duration = Duration::from_secs(10);
/// Wall-clock time beyond monotonic time that counts as a sleep.
pub const RESUME_GAP: Duration = Duration::from_secs(5);

/// Start the watchers on the service's runtime.
pub(crate) fn spawn(service: SessionService) {
    let (tx, rx) = mpsc::channel(64);
    let handle = service.handle().clone();
    handle.spawn(watch_interfaces(tx.clone()));
    handle.spawn(watch_resume(service.clone(), tx));
    let clock = service.clock();
    handle.spawn(debounce(rx, NETWORK_DEBOUNCE, clock, move || {
        let service = service.clone();
        async move {
            service.network_changed().await;
        }
    }));
}

/// Forward OS interface events as triggers.
async fn watch_interfaces(tx: mpsc::Sender<()>) {
    let mut watcher = match if_watch::tokio::IfWatcher::new() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "network-change events unavailable; relying on resume checks and keepalives");
            return;
        }
    };
    loop {
        match std::future::poll_fn(|cx| watcher.poll_if_event(cx)).await {
            Ok(event) => {
                tracing::debug!(?event, "network interface event");
                if tx.send(()).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "network-change watcher stopped");
                return;
            }
        }
    }
}

/// Whether the wall clock ran ahead of the monotonic clock by more than
/// `gap`, which means the machine was suspended in between.
pub fn slept(wall_elapsed: Duration, monotonic_elapsed: Duration, gap: Duration) -> bool {
    wall_elapsed > monotonic_elapsed + gap
}

/// Trigger when the machine resumes, checking only while a connection is
/// open so an idle runtime has no periodic wakeups.
async fn watch_resume(service: SessionService, tx: mpsc::Sender<()>) {
    let mut events = service.subscribe();
    loop {
        if service.connections().is_empty() {
            // Wait for any session activity, then look again.
            if let Err(tokio::sync::broadcast::error::RecvError::Closed) = events.recv().await {
                return;
            }
            continue;
        }
        // Real clocks on purpose: this measures the machine's own sleep.
        let (wall, mono) = (SystemTime::now(), Instant::now());
        tokio::time::sleep(RESUME_CHECK_INTERVAL).await;
        let wall_elapsed = SystemTime::now().duration_since(wall).unwrap_or_default();
        if slept(wall_elapsed, mono.elapsed(), RESUME_GAP) {
            tracing::info!(
                gap_s = (wall_elapsed.saturating_sub(mono.elapsed())).as_secs(),
                "resume from sleep detected"
            );
            if tx.send(()).await.is_err() {
                return;
            }
        }
    }
}

/// Run `fire` once after each burst of triggers has been quiet for `quiet`.
pub(crate) async fn debounce<F, Fut>(
    mut rx: mpsc::Receiver<()>,
    quiet: Duration,
    clock: Arc<dyn Clock>,
    mut fire: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    while rx.recv().await.is_some() {
        loop {
            tokio::select! {
                more = rx.recv() => {
                    if more.is_none() {
                        return;
                    }
                }
                () = clock.sleep(quiet) => break,
            }
        }
        fire().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::clock::ManualClock;

    #[test]
    fn a_wall_clock_jump_beyond_the_gap_is_a_sleep() {
        let s = Duration::from_secs;
        assert!(!slept(s(10), s(10), RESUME_GAP));
        assert!(!slept(s(14), s(10), RESUME_GAP));
        assert!(slept(s(16), s(10), RESUME_GAP));
        assert!(slept(s(3600), s(10), RESUME_GAP));
        // The wall clock stepping back (NTP) is not a sleep.
        assert!(!slept(Duration::ZERO, s(10), RESUME_GAP));
    }

    #[tokio::test]
    async fn bursts_of_events_fire_once_after_the_quiet_period() {
        let clock = Arc::new(ManualClock::new());
        let fired = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel(8);
        let f = Arc::clone(&fired);
        let task = tokio::spawn(debounce(
            rx,
            NETWORK_DEBOUNCE,
            Arc::clone(&clock) as Arc<dyn Clock>,
            move || {
                let f = Arc::clone(&f);
                async move {
                    f.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));
        let settle = || async {
            for _ in 0..20 {
                tokio::task::yield_now().await;
            }
        };
        for _ in 0..5 {
            tx.send(()).await.unwrap();
            settle().await;
            clock.advance(Duration::from_millis(50));
        }
        settle().await;
        assert_eq!(fired.load(Ordering::SeqCst), 0, "still inside the burst");
        clock.advance(Duration::from_millis(60));
        settle().await;
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        tx.send(()).await.unwrap();
        settle().await;
        clock.advance(NETWORK_DEBOUNCE);
        settle().await;
        assert_eq!(fired.load(Ordering::SeqCst), 2);
        drop(tx);
        task.await.unwrap();
    }
}
