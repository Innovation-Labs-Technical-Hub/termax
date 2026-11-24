//! Terminal-core measurements. Every function takes a backend factory so the
//! same numbers can be produced for each `TermCore` implementation.

use std::path::PathBuf;
use std::sync::mpsc::{RecvTimeoutError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use oxt_term::{Budget, Frame, GridSize, TermConfig, TermCore, TermEvent};

use crate::metric::{self, DEFAULT_TOLERANCE, Metric};
use crate::workloads;

/// Parser throughput target on its own thread.
const TARGET_PARSER_MB_S: f64 = 300.0;
/// Extra memory per 80×24 tab holding 10k lines of ASCII scrollback.
const TARGET_TAB_MIB: f64 = 3.0;
/// CPU budget for a full 200×60 redraw; the snapshot is only part of it.
const TARGET_FULL_SNAPSHOT_MS: f64 = 1.5;
/// Budget for a one-line change (the 1-line scroll target).
const TARGET_LINE_SNAPSHOT_MS: f64 = 0.2;
/// Typed input must show up within this time while output floods the screen.
const TARGET_FLOOD_ECHO_MS: f64 = 50.0;

/// Size of each chunk the I/O side hands to the parser.
const IO_CHUNK: usize = 64 * 1024;
/// Display refresh the UI pulls frames at.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
/// How often a "keystroke echo" marker is injected into the flood.
const ECHO_EVERY: Duration = Duration::from_millis(20);
const FLOOD_RUN: Duration = Duration::from_secs(2);

/// Builds a terminal for one measurement.
pub type Factory = fn(TermConfig) -> Box<dyn TermCore>;

pub struct Options {
    pub backend_name: &'static str,
    pub factory: Factory,
    pub check: bool,
    pub json: Option<PathBuf>,
    pub baseline: Option<PathBuf>,
    pub tolerance: f64,
    pub bytes: usize,
    pub tabs: usize,
    pub ssh: bool,
}

impl Options {
    pub fn default_tolerance() -> f64 {
        DEFAULT_TOLERANCE
    }
}

pub fn run(opts: &Options) -> Result<bool> {
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are not meaningful (use `cargo xtask perf`)");
    }
    println!("backend: {}\n", opts.backend_name);
    let make = opts.factory;
    let mut metrics = Vec::new();

    for (name, data, target) in [
        (
            "parser ascii",
            workloads::ascii_lines(opts.bytes),
            Some(TARGET_PARSER_MB_S),
        ),
        ("parser sgr-heavy", workloads::sgr_heavy(opts.bytes), None),
        ("parser unicode", workloads::unicode_mix(opts.bytes), None),
    ] {
        let m = Metric::higher(name, parser_throughput(make, &data), "MB/s");
        metrics.push(match target {
            Some(t) => m.target(t),
            None => m,
        });
    }

    let (full_ms, line_ms) = snapshot_costs(make);
    metrics.push(
        Metric::lower("snapshot full 200x60", full_ms, "ms")
            .target(TARGET_FULL_SNAPSHOT_MS)
            .abs_slack(0.02),
    );
    metrics.push(
        Metric::lower("snapshot one line 200x60", line_ms, "ms")
            .target(TARGET_LINE_SNAPSHOT_MS)
            .abs_slack(0.01),
    );

    for ring_mib in [1, 4] {
        let (p50, p99) = flood_echo_latency(make, ring_mib << 20);
        metrics.push(
            Metric::lower(
                format!("echo under flood p50, {ring_mib} MiB window"),
                p50,
                "ms",
            )
            .abs_slack(5.0),
        );
        metrics.push(
            Metric::lower(
                format!("echo under flood p99, {ring_mib} MiB window"),
                p99,
                "ms",
            )
            .target(TARGET_FLOOD_ECHO_MS)
            .abs_slack(5.0),
        );
    }

    for (label, lines, target) in [
        ("empty history", 0, None),
        ("1k lines", 1_000, None),
        ("10k lines", 10_000, Some(TARGET_TAB_MIB)),
    ] {
        let (rss_mib, estimate_mib) = memory_per_tab(make, opts.tabs, lines)?;
        // RSS includes allocator reuse and varies run to run, so it is
        // reported (and held to the target) but only the core's own
        // estimate, which is deterministic, is gated.
        let m = Metric::lower(
            format!("memory per tab 80x24, {label} (RSS)"),
            rss_mib,
            "MiB",
        )
        .ungated();
        metrics.push(match target {
            Some(t) => m.target(t),
            None => m,
        });
        metrics.push(Metric::lower(
            format!("memory per tab 80x24, {label} (estimate)"),
            estimate_mib,
            "MiB",
        ));
    }

    if opts.ssh {
        crate::ssh_perf::run(&mut metrics)?;
    }

    let baseline = opts
        .baseline
        .as_deref()
        .map(metric::load_baseline)
        .transpose()?;
    let verdict = metric::report(&metrics, baseline.as_ref(), opts.tolerance);
    if let Some(path) = &opts.json {
        std::fs::write(path, serde_json::to_vec_pretty(&metrics)?)
            .with_context(|| format!("writing {}", path.display()))?;
    }
    println!(
        "\n{} target(s) missed, {} regression(s){}",
        verdict.targets_missed,
        verdict.regressions,
        if baseline.is_some() {
            ""
        } else {
            " (no baseline given)"
        }
    );
    Ok(verdict.regressions == 0 && (!opts.check || verdict.targets_missed == 0))
}

/// Feed `data` in default-sized budgets (as the per-session parser task
/// does) and return MB/s, best of three runs.
fn parser_throughput(make: Factory, data: &[u8]) -> f64 {
    let budget = Budget::default();
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let mut term = make(TermConfig {
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
fn snapshot_costs(make: Factory) -> (f64, f64) {
    let mut term = make(TermConfig {
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
        term.snapshot(&mut frame);
        line.push(t.elapsed());
    }
    (median_ms(&mut full), median_ms(&mut line))
}

/// Time from "remote echo sent" to "visible in a published frame" while the
/// host floods the terminal, with the parser on its own thread behind a
/// bounded input buffer the size of the SSH receive window (flow control
/// keeps at most that much output queued ahead of the echo).
///
/// Returns (p50, p99) in milliseconds.
fn flood_echo_latency(make: Factory, window_bytes: usize) -> (f64, f64) {
    let flood: Arc<[u8]> = workloads::ascii_lines(8 << 20).into();
    let (tx, rx) = sync_channel::<Vec<u8>>((window_bytes / IO_CHUNK).max(1));
    let sent_at: Arc<Mutex<Vec<Instant>>> = Arc::default();

    let producer = {
        let sent_at = Arc::clone(&sent_at);
        thread::spawn(move || {
            let start = Instant::now();
            let mut next_echo = start + ECHO_EVERY;
            let mut offset = 0;
            while start.elapsed() < FLOOD_RUN {
                let end = (offset + IO_CHUNK).min(flood.len());
                if tx.send(flood[offset..end].to_vec()).is_err() {
                    return;
                }
                offset = if end == flood.len() { 0 } else { end };
                if Instant::now() >= next_echo {
                    let mut stamps = sent_at.lock().unwrap();
                    let marker = format!("\x1b]0;E{}\x07", stamps.len());
                    stamps.push(Instant::now());
                    drop(stamps);
                    if tx.send(marker.into_bytes()).is_err() {
                        return;
                    }
                    next_echo += ECHO_EVERY;
                }
            }
        })
    };

    let mut term = make(TermConfig {
        size: GridSize::new(120, 40),
        ..TermConfig::default()
    });
    let mut frame = Frame::default();
    let mut events = Vec::new();
    let mut replies = Vec::new();
    let mut parsed_unshown: Vec<usize> = Vec::new();
    let mut latencies = Vec::new();
    let mut last_frame = Instant::now();

    loop {
        match rx.recv_timeout(FRAME_INTERVAL) {
            Ok(chunk) => {
                let mut rest = chunk.as_slice();
                while !rest.is_empty() {
                    let fed = term.feed(rest, Budget::default());
                    rest = &rest[fed.consumed..];
                    term.drain_replies(&mut replies);
                    replies.clear();
                }
                term.take_events(&mut events);
                for e in events.drain(..) {
                    if let TermEvent::Title(t) = e
                        && let Some(n) = t.strip_prefix('E').and_then(|n| n.parse().ok())
                    {
                        parsed_unshown.push(n);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // The UI pulls a frame once per display refresh.
        if last_frame.elapsed() >= FRAME_INTERVAL {
            term.snapshot(&mut frame);
            last_frame = Instant::now();
            let stamps = sent_at.lock().unwrap();
            for n in parsed_unshown.drain(..) {
                latencies.push(last_frame - stamps[n]);
            }
        }
    }
    producer.join().expect("flood producer panicked");

    if latencies.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    latencies.sort();
    let pct = |p: f64| {
        let i = ((latencies.len() as f64 * p).ceil() as usize).clamp(1, latencies.len()) - 1;
        latencies[i].as_secs_f64() * 1e3
    };
    (pct(0.50), pct(0.99))
}

fn median_ms(samples: &mut [Duration]) -> f64 {
    samples.sort();
    samples[samples.len() / 2].as_secs_f64() * 1e3
}

/// Average extra memory per 80×24 terminal holding `lines` lines of ASCII
/// history plus a screenful of prompt output.
fn memory_per_tab(make: Factory, tabs: usize, lines: usize) -> Result<(f64, f64)> {
    let tabs = tabs.max(1);
    let mut history = Vec::new();
    for i in 0..lines.max(1) {
        history.extend_from_slice(format!("{i:>8} {}\r\n", "x".repeat(71)).as_bytes());
    }
    history.extend_from_slice(b"user@host:~$ ");

    let before = rss()?;
    let mut terms = Vec::with_capacity(tabs);
    for _ in 0..tabs {
        let mut term = make(TermConfig {
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
