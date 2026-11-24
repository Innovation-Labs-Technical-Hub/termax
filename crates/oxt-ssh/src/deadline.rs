//! Connect deadline that excludes time spent waiting on the user.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

use crate::error::{Error, Result};

/// Tracks how long the user has been prompted so the connect budget only
/// counts network and crypto time.
#[derive(Debug, Default)]
pub(crate) struct PromptClock {
    state: Mutex<ClockState>,
}

#[derive(Debug, Default)]
struct ClockState {
    accumulated: Duration,
    prompting_since: Option<Instant>,
    depth: u32,
}

impl PromptClock {
    fn lock(&self) -> std::sync::MutexGuard<'_, ClockState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Total paused time so far (including an ongoing prompt).
    fn paused(&self) -> Duration {
        let s = self.lock();
        s.accumulated + s.prompting_since.map_or(Duration::ZERO, |t| t.elapsed())
    }

    fn begin(&self) {
        let mut s = self.lock();
        if s.depth == 0 {
            s.prompting_since = Some(Instant::now());
        }
        s.depth += 1;
    }

    fn end(&self) {
        let mut s = self.lock();
        s.depth = s.depth.saturating_sub(1);
        if s.depth == 0
            && let Some(t) = s.prompting_since.take()
        {
            s.accumulated += t.elapsed();
        }
    }

    /// Run a user prompt with the deadline paused.
    pub(crate) async fn pause<T>(&self, fut: impl Future<Output = T>) -> T {
        struct Guard<'a>(&'a PromptClock);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                self.0.end();
            }
        }
        self.begin();
        let _g = Guard(self);
        fut.await
    }

    /// Run `fut` with a budget of `budget` plus any prompt time.
    pub(crate) async fn run<T>(
        &self,
        stage: &'static str,
        budget: Duration,
        fut: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        let start = Instant::now();
        tokio::pin!(fut);
        loop {
            let deadline = start + budget + self.paused();
            tokio::select! {
                r = &mut fut => return r,
                () = tokio::time::sleep_until(deadline) => {
                    if Instant::now() >= start + budget + self.paused() {
                        return Err(Error::Timeout { stage, after: budget });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn times_out_without_prompts() {
        let c = PromptClock::default();
        let r = c
            .run("t", Duration::from_secs(1), async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok(())
            })
            .await;
        assert!(matches!(r, Err(Error::Timeout { .. })));
    }

    #[tokio::test(start_paused = true)]
    async fn prompt_time_is_not_counted() {
        let c = PromptClock::default();
        let r = c
            .run("t", Duration::from_secs(2), async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                c.pause(tokio::time::sleep(Duration::from_secs(10))).await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                Ok(7)
            })
            .await;
        assert_eq!(r.unwrap(), 7);
    }
}
