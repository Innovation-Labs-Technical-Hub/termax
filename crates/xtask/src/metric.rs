//! Measured values, their targets, and regression checks against a baseline.
//!
//! Two different questions are kept apart:
//! - **Target**: does the value meet the goal in the architecture brief?
//!   Reported always; enforced only with `--check`.
//! - **Regression**: is the value meaningfully worse than a baseline run on
//!   the same machine? Enforced whenever `--baseline` is given, so CI can
//!   block slowdowns even while a target is still out of reach.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Relative slack allowed before a gated metric counts as a regression.
pub const DEFAULT_TOLERANCE: f64 = 0.10;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub higher_is_better: bool,
    /// Goal from the architecture brief, if any.
    pub target: Option<f64>,
    /// Whether the value is stable enough to fail CI on regressions.
    pub gated: bool,
    /// Absolute slack added to the relative tolerance, for tiny values
    /// where timer resolution dominates.
    pub abs_slack: f64,
}

impl Metric {
    fn new(name: impl Into<String>, value: f64, unit: &str, higher_is_better: bool) -> Self {
        Self {
            name: name.into(),
            value,
            unit: unit.into(),
            higher_is_better,
            target: None,
            gated: true,
            abs_slack: 0.0,
        }
    }

    /// A metric where larger values are better (throughput).
    pub fn higher(name: impl Into<String>, value: f64, unit: &str) -> Self {
        Self::new(name, value, unit, true)
    }

    /// A metric where smaller values are better (time, memory).
    pub fn lower(name: impl Into<String>, value: f64, unit: &str) -> Self {
        Self::new(name, value, unit, false)
    }

    pub fn target(mut self, target: f64) -> Self {
        self.target = Some(target);
        self
    }

    /// Report only; too noisy (or environment-dependent) to gate on.
    pub fn ungated(mut self) -> Self {
        self.gated = false;
        self
    }

    pub fn abs_slack(mut self, slack: f64) -> Self {
        self.abs_slack = slack;
        self
    }

    pub fn meets_target(&self) -> Option<bool> {
        self.target.map(|t| {
            if self.higher_is_better {
                self.value >= t
            } else {
                self.value <= t
            }
        })
    }

    /// Change versus `base` in percent, positive meaning better.
    fn improvement_pct(&self, base: f64) -> f64 {
        if base == 0.0 {
            return 0.0;
        }
        let change = (self.value - base) / base * 100.0;
        if self.higher_is_better {
            change
        } else {
            -change
        }
    }

    fn regressed(&self, base: f64, tolerance: f64) -> bool {
        if !self.gated {
            return false;
        }
        if self.higher_is_better {
            self.value < base * (1.0 - tolerance) - self.abs_slack
        } else {
            self.value > base * (1.0 + tolerance) + self.abs_slack
        }
    }
}

/// One harness run as written by `--json`.
#[derive(Serialize, Deserialize)]
pub struct Results {
    /// Terminal backend the terminal metrics were measured on.
    pub backend: String,
    pub metrics: Vec<Metric>,
}

/// Backend assumed for baselines written before results recorded it; only
/// alacritty_terminal existed then.
const LEGACY_BACKEND: &str = "alacritty_terminal";

/// A baseline run: its backend and values by metric name.
pub struct Baseline {
    pub backend: String,
    pub values: HashMap<String, f64>,
}

pub fn load_baseline(path: &Path) -> Result<Baseline> {
    let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let results = match serde_json::from_slice::<Results>(&raw) {
        Ok(r) => r,
        Err(_) => Results {
            backend: LEGACY_BACKEND.into(),
            metrics: serde_json::from_slice(&raw)
                .with_context(|| format!("parsing {}", path.display()))?,
        },
    };
    Ok(Baseline {
        backend: results.backend,
        values: results
            .metrics
            .into_iter()
            .map(|m| (m.name, m.value))
            .collect(),
    })
}

/// Outcome of a run.
pub struct Verdict {
    pub targets_missed: usize,
    pub regressions: usize,
}

/// Print the results table and count target misses and regressions.
pub fn report(
    metrics: &[Metric],
    baseline: Option<&HashMap<String, f64>>,
    tolerance: f64,
) -> Verdict {
    let mut verdict = Verdict {
        targets_missed: 0,
        regressions: 0,
    };
    println!(
        "{:<44} {:>11} {:<5} {:>9} {:<6} {:>9} {:<10}",
        "metric", "value", "unit", "target", "", "vs base", ""
    );
    for m in metrics {
        let (target, target_result) = match (m.target, m.meets_target()) {
            (Some(t), Some(ok)) => {
                if !ok {
                    verdict.targets_missed += 1;
                }
                let op = if m.higher_is_better { ">=" } else { "<=" };
                (format!("{op}{t}"), if ok { "met" } else { "MISSED" })
            }
            _ => ("-".into(), ""),
        };
        let (vs_base, base_result) = match baseline.and_then(|b| b.get(&m.name)) {
            Some(&base) => {
                let regressed = m.regressed(base, tolerance);
                if regressed {
                    verdict.regressions += 1;
                }
                let label = match (m.gated, regressed) {
                    (false, _) => "info",
                    (true, true) => "REGRESSED",
                    (true, false) => "ok",
                };
                (format!("{:+.1}%", m.improvement_pct(base)), label)
            }
            None => ("-".into(), ""),
        };
        println!(
            "{:<44} {:>11.3} {:<5} {:>9} {:<6} {:>9} {:<10}",
            m.name, m.value, m.unit, target, target_result, vs_base, base_result
        );
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_respects_direction_tolerance_and_slack() {
        let fast = Metric::higher("t", 89.0, "MB/s");
        assert!(fast.regressed(100.0, 0.10));
        assert!(!Metric::higher("t", 91.0, "MB/s").regressed(100.0, 0.10));

        let slow = Metric::lower("l", 11.5, "ms");
        assert!(slow.regressed(10.0, 0.10));
        assert!(!Metric::lower("l", 10.9, "ms").regressed(10.0, 0.10));

        let tiny = Metric::lower("s", 0.004, "ms").abs_slack(0.01);
        assert!(!tiny.regressed(0.002, 0.10), "timer noise must not fail CI");

        assert!(
            !Metric::higher("n", 1.0, "x")
                .ungated()
                .regressed(100.0, 0.10)
        );
    }

    #[test]
    fn baselines_record_backend_and_accept_legacy_arrays() {
        let dir = std::env::temp_dir().join(format!("xtask-baseline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = Metric::higher("parser ascii", 150.0, "MB/s");

        let legacy = dir.join("legacy.json");
        std::fs::write(&legacy, serde_json::to_vec(&vec![m.clone()]).unwrap()).unwrap();
        let b = load_baseline(&legacy).unwrap();
        assert_eq!(b.backend, LEGACY_BACKEND);
        assert_eq!(b.values["parser ascii"], 150.0);

        let current = dir.join("current.json");
        let results = Results {
            backend: "libghostty-vt".into(),
            metrics: vec![m],
        };
        std::fs::write(&current, serde_json::to_vec(&results).unwrap()).unwrap();
        assert_eq!(load_baseline(&current).unwrap().backend, "libghostty-vt");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn target_and_improvement_signs() {
        let m = Metric::lower("mem", 2.0, "MiB").target(3.0);
        assert_eq!(m.meets_target(), Some(true));
        assert!(
            m.improvement_pct(4.0) > 0.0,
            "less memory is an improvement"
        );
        assert_eq!(Metric::higher("x", 1.0, "x").meets_target(), None);
    }
}
