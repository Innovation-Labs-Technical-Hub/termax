//! Reconnect backoff: 1, 2, 4, 8, 16, 32 s, capped at 60 s, with jitter.

use std::time::Duration;

/// Default first wait.
pub const DEFAULT_BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Default longest wait.
pub const DEFAULT_BACKOFF_CAP: Duration = Duration::from_secs(60);
/// Default jitter: each wait is scaled by a uniform factor in `1 ± 0.2`.
pub const DEFAULT_BACKOFF_JITTER: f64 = 0.2;
/// Default number of attempts before giving up.
pub const DEFAULT_RECONNECT_ATTEMPTS: u32 = 10;

/// When and how often a dropped session is reconnected.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconnectPolicy {
    /// Reconnect at all.
    pub enabled: bool,
    /// Wait before the first attempt; doubled for each further attempt.
    pub base: Duration,
    /// Longest wait, jitter included.
    pub cap: Duration,
    /// Relative jitter in `[0, 1)`.
    pub jitter: f64,
    /// Attempts before giving up; `None` retries forever.
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            base: DEFAULT_BACKOFF_BASE,
            cap: DEFAULT_BACKOFF_CAP,
            jitter: DEFAULT_BACKOFF_JITTER,
            max_attempts: Some(DEFAULT_RECONNECT_ATTEMPTS),
        }
    }
}

impl ReconnectPolicy {
    /// Check ranges.
    pub fn validate(&self) -> Result<(), String> {
        if self.base.is_zero() || self.cap < self.base {
            return Err("reconnect backoff needs 0 < base <= cap".into());
        }
        if !(0.0..1.0).contains(&self.jitter) {
            return Err("reconnect jitter must be in [0, 1)".into());
        }
        if self.max_attempts == Some(0) {
            return Err("reconnect max_attempts must be at least 1 (or unlimited)".into());
        }
        Ok(())
    }

    /// Wait before 1-based `attempt` without jitter: `base · 2^(attempt-1)`,
    /// at most `cap`.
    pub fn nominal_delay(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1).min(31);
        self.base.saturating_mul(1 << exp).min(self.cap)
    }

    /// Wait before `attempt`, with `unit` a uniform sample in `[0, 1)` that
    /// picks the jitter.
    pub fn delay(&self, attempt: u32, unit: f64) -> Duration {
        let factor = 1.0 + self.jitter * (2.0 * unit.clamp(0.0, 1.0) - 1.0);
        self.nominal_delay(attempt).mul_f64(factor).min(self.cap)
    }

    /// Whether `attempt` may run.
    pub fn allows(&self, attempt: u32) -> bool {
        self.enabled && self.max_attempts.is_none_or(|m| attempt <= m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_doubles_to_the_cap_with_bounded_jitter() {
        let p = ReconnectPolicy::default();
        p.validate().unwrap();
        let nominal: Vec<u64> = (1..=10).map(|a| p.nominal_delay(a).as_secs()).collect();
        assert_eq!(nominal, [1, 2, 4, 8, 16, 32, 60, 60, 60, 60]);
        for attempt in 1..=10 {
            let n = p.nominal_delay(attempt).as_secs_f64();
            let lo = p.delay(attempt, 0.0).as_secs_f64();
            let hi = p.delay(attempt, 0.999_999).as_secs_f64();
            assert!((lo - n * 0.8).abs() < 1e-3, "attempt {attempt}: {lo}");
            assert!(hi <= (n * 1.2).min(60.0) + 1e-3, "attempt {attempt}: {hi}");
            assert!(hi <= 60.0);
        }
        assert!(p.allows(10) && !p.allows(11));
        let unlimited = ReconnectPolicy {
            max_attempts: None,
            ..ReconnectPolicy::default()
        };
        assert!(unlimited.allows(1000));
        assert!(
            !ReconnectPolicy {
                enabled: false,
                ..ReconnectPolicy::default()
            }
            .allows(1)
        );
    }

    #[test]
    fn invalid_policies_are_rejected() {
        let bad = [
            ReconnectPolicy {
                jitter: 1.0,
                ..ReconnectPolicy::default()
            },
            ReconnectPolicy {
                base: Duration::ZERO,
                ..ReconnectPolicy::default()
            },
            ReconnectPolicy {
                cap: Duration::from_millis(10),
                ..ReconnectPolicy::default()
            },
            ReconnectPolicy {
                max_attempts: Some(0),
                ..ReconnectPolicy::default()
            },
        ];
        for p in bad {
            assert!(p.validate().is_err(), "{p:?}");
        }
    }
}
