//! Terminal-core measurements and their budgets.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use oxt_term::{AlacrittyTerm, Budget, Frame, GridSize, TermConfig, TermCore};
use serde::Serialize;

use crate::workloads;

/// Parser throughput target on its own thread.
const MIN_PARSER_MB_S: f64 = 300.0;
/// Extra memory per 80×24 tab holding 10k lines of ASCII scrollback.
const MAX_TAB_MIB: f64 = 3.0;
/// CPU budget for a full 200×60 redraw. The renderer's share is not measured
/// here, so the snapshot must fit well inside it.
const MAX_FULL_SNAPSHOT_MS: f64 = 1.5;
/// Budget for a one-line change (the 1-line scroll target).
const MAX_LINE_SNAPSHOT_MS: f64 = 0.2;

pub struct Options {
    pub check: bool,
    pub json: Option<PathBuf>,
    pub bytes: usize,
    pub tabs: usize,
}

#[derive(Serialize)]
struct Metric {
    name: String,
    value: f64,
    unit: &'static str,
    /// `None` when the metric is informational only.
    budget: Option<Budget_>,
    pass: bool,
}

#[derive(Serialize, Clone, Copy)]
struct Budget_ {
    limit: f64,
    higher_is_better: bool,
}

impl Metric {
    fn new(
        name: impl Into<String>,
        value: f64,
        unit: &'static str,
        budget: Option<Budget_>,
    ) -> Self {
        let pass = budget.is_none_or(|b| {
            if b.higher_is_better {
                value >= b.limit
            } else {
                value <= b.limit
            }
        });
        Self {
            name: name.into(),
            value,
            unit,
            budget,
            pass,
        }
    }
}

fn at_least(limit: f64) -> Option<Budget_> {
    Some(Budget_ {
        limit,
        higher_is_better: true,
    })
}

fn at_most(limit: f64) -> Option<Budget_> {
    Some(Budget_ {
        limit,
        higher_is_better: false,
    })
}

pub fn run(opts: &Options) -> Result<bool> {
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are not meaningful (use `cargo xtask perf`)");
    }
    let mut metrics = Vec::new();

    for (name, data) in [
        ("parser ascii", workloads::ascii_lines(opts.bytes)),
        ("parser sgr-heavy", workloads::sgr_heavy(opts.bytes)),
        ("parser unicode", workloads::unicode_mix(opts.bytes)),
    ] {
        let mb_s = parser_throughput(&data);
        // Only plain ASCII carries the hard target; the others show how much
        // escape and Unicode handling costs relative to it.
        let budget = (name == "parser ascii")
            .then_some(())
            .and(at_least(MIN_PARSER_MB_S));
        metrics.push(Metric::new(name, mb_s, "MB/s", budget));
    }

    let (full_ms, line_ms) = snapshot_costs();
    metrics.push(Metric::new(
        "snapshot full 200x60",
        full_ms,
        "ms",
        at_most(MAX_FULL_SNAPSHOT_MS),
    ));
    metrics.push(Metric::new(
        "snapshot one line 200x60",
        line_ms,
        "ms",
        at_most(MAX_LINE_SNAPSHOT_MS),
    ));

    let (rss_mib, estimate_mib) = memory_per_tab(opts.tabs)?;
    metrics.push(Metric::new(
        "memory per tab (RSS) 80x24 + 10k lines",
        rss_mib,
        "MiB",
        at_most(MAX_TAB_MIB),
    ));
    metrics.push(Metric::new(
        "memory per tab (core estimate)",
        estimate_mib,
        "MiB",
        None,
    ));

    print_table(&metrics);
    if let Some(path) = &opts.json {
        std::fs::write(path, serde_json::to_vec_pretty(&metrics)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    let all_pass = metrics.iter().all(|m| m.pass);
    Ok(!opts.check || all_pass)
}

/// Feed `data` in default-sized budgets (as the per-session parser task
/// does) and return MB/s, best of three runs.
fn parser_throughput(data: &[u8]) -> f64 {
    let budget = Budget::default();
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let mut term = AlacrittyTerm::new(TermConfig {
            size: GridSize::new(120, 40),
            ..TermConfig::default()
        });
        let mut replies = Vec::new();
        let start = Instant::now();
        let mut rest = data;
        while !rest.is_empty() {
            let fed = term.feed(rest, budget);
            rest = &rest[fed.consumed..];
            term.drain_replies(&mut replies);
            replies.clear();
        }
        best = best.min(start.elapsed());
    }
    data.len() as f64 / 1e6 / best.as_secs_f64()
}

/// Median cost of a full-screen snapshot and of a snapshot after one line changed.
fn snapshot_costs() -> (f64, f64) {
    let mut term = AlacrittyTerm::new(TermConfig {
        size: GridSize::new(200, 60),
        ..TermConfig::default()
    });
    let fill = workloads::sgr_heavy(200 * 60 * 4);
    term.feed(&fill, Budget::UNLIMITED);
    let mut frame = Frame::default();
    term.snapshot(&mut frame);

    let mut full = Vec::new();
    let mut line = Vec::new();
    for i in 0..200 {
        // Clearing the screen forces full damage.
        term.feed(b"\x1b[H\x1b[2J", Budget::UNLIMITED);
        term.feed(&fill[..200 * 60], Budget::UNLIMITED);
        let t = Instant::now();
        term.snapshot(&mut frame);
        full.push(t.elapsed());

        // Rewrite one row in place, as a prompt or progress bar does.
        term.feed(
            format!("\x1b[30;1Hline {i:>6}").as_bytes(),
            Budget::UNLIMITED,
        );
        let t = Instant::now();
        let damage = term.snapshot(&mut frame);
        line.push(t.elapsed());
        debug_assert!(!damage.full);
    }
    (median_ms(&mut full), median_ms(&mut line))
}

fn median_ms(samples: &mut [Duration]) -> f64 {
    samples.sort();
    samples[samples.len() / 2].as_secs_f64() * 1e3
}

/// Average extra memory per 80×24 terminal holding 10k lines of ASCII history.
fn memory_per_tab(tabs: usize) -> Result<(f64, f64)> {
    let tabs = tabs.max(1);
    // Exactly 10k 80-column lines so each tab's history is full.
    let mut history = Vec::new();
    for i in 0..10_000 {
        history.extend_from_slice(format!("{i:>8} {}\r\n", "x".repeat(71)).as_bytes());
    }

    let before = rss()?;
    let mut terms = Vec::with_capacity(tabs);
    for _ in 0..tabs {
        let mut term = AlacrittyTerm::new(TermConfig {
            size: GridSize::new(80, 24),
            scrollback_lines: 10_000,
            ..TermConfig::default()
        });
        term.feed(&history, Budget::UNLIMITED);
        let mut frame = Frame::default();
        term.snapshot(&mut frame);
        terms.push((term, frame));
    }
    let after = rss()?;
    let estimate: usize = terms.iter().map(|(t, _)| t.memory_estimate()).sum();
    let mib = |b: f64| b / tabs as f64 / (1024.0 * 1024.0);
    Ok((
        mib(after.saturating_sub(before) as f64),
        mib(estimate as f64),
    ))
}

fn rss() -> Result<usize> {
    memory_stats::memory_stats()
        .map(|m| m.physical_mem)
        .context("resident memory is not available on this platform")
}

fn print_table(metrics: &[Metric]) {
    println!(
        "{:<42} {:>12}  {:<6} {:>14}  result",
        "metric", "value", "unit", "budget"
    );
    for m in metrics {
        let budget = match m.budget {
            Some(b) if b.higher_is_better => format!(">= {}", b.limit),
            Some(b) => format!("<= {}", b.limit),
            None => "-".into(),
        };
        let result = match (m.budget, m.pass) {
            (None, _) => "info",
            (Some(_), true) => "pass",
            (Some(_), false) => "FAIL",
        };
        println!(
            "{:<42} {:>12.3}  {:<6} {:>14}  {result}",
            m.name, m.value, m.unit, budget
        );
    }
}
