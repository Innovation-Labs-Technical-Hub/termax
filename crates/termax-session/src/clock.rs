//! Injectable time source. Backoff, idle compaction, DNS and connect
//! timeouts and event timestamps all go through [`Clock`], so tests can run
//! them on a [`ManualClock`].

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio::sync::watch;

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Monotonic time and timers.
pub trait Clock: Send + Sync + 'static {
    /// Current time.
    fn now(&self) -> Instant;

    /// Completes once [`Clock::now`] has reached `deadline`.
    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()>;

    /// Completes after `duration`.
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.sleep_until(self.now() + duration)
    }
}

/// The real monotonic clock, with tokio timers (one timer wheel per runtime,
/// armed only while something waits on it).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep_until(deadline.into()))
    }
}

/// A clock that only moves when [`ManualClock::advance`] is called.
#[derive(Debug)]
pub struct ManualClock {
    base: Instant,
    offset: watch::Sender<Duration>,
}

impl Default for ManualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualClock {
    /// A clock frozen at the current instant.
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            offset: watch::Sender::new(Duration::ZERO),
        }
    }

    /// Move time forward, waking every sleeper whose deadline has passed.
    pub fn advance(&self, by: Duration) {
        self.offset.send_modify(|o| *o += by);
    }

    /// Number of timers currently waiting on this clock.
    pub fn sleepers(&self) -> usize {
        self.offset.receiver_count()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.base + *self.offset.borrow()
    }

    fn sleep_until(&self, deadline: Instant) -> BoxFuture<'static, ()> {
        let mut rx = self.offset.subscribe();
        let base = self.base;
        Box::pin(async move {
            loop {
                if base + *rx.borrow_and_update() >= deadline {
                    return;
                }
                if rx.changed().await.is_err() {
                    // The clock is gone; time never advances again.
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn manual_clock_wakes_sleepers_only_when_advanced_past_deadline() {
        let clock = Arc::new(ManualClock::new());
        let start = clock.now();
        let c = Arc::clone(&clock);
        let sleeper = tokio::spawn(async move { c.sleep(Duration::from_secs(5)).await });
        while clock.sleepers() == 0 {
            tokio::task::yield_now().await;
        }
        clock.advance(Duration::from_secs(4));
        tokio::task::yield_now().await;
        assert!(!sleeper.is_finished());
        clock.advance(Duration::from_secs(1));
        sleeper.await.unwrap();
        assert_eq!(clock.now() - start, Duration::from_secs(5));
        assert_eq!(clock.sleepers(), 0);
    }
}
