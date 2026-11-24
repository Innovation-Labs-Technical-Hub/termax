//! Measures one backend with the same method as `cargo xtask perf`.
//!
//! ```text
//! cargo run --release -p termax-term-ghostty --example backend_compare -- ghostty
//! cargo run --release -p termax-term-ghostty --example backend_compare -- alacritty
//! ```
//!
//! Run one backend per process so freed memory from one cannot hide the
//! other's RSS growth.

use std::time::{Duration, Instant};

use termax_term::{AlacrittyTerm, Budget, Frame, GridSize, TermConfig, TermCore};
use termax_term_ghostty::GhosttyTerm;

const MIB: usize = 1 << 20;
const TABS: usize = 20;

#[derive(Clone, Copy, PartialEq)]
enum Backend {
    Alacritty,
    Ghostty,
    /// Ghostty with its scrollback compression run once the tab is idle.
    GhosttyCompressed,
}

impl Backend {
    fn make(self, config: TermConfig) -> Box<dyn TermCore> {
        match self {
            Backend::Alacritty => Box::new(AlacrittyTerm::new(config)),
            Backend::Ghostty | Backend::GhosttyCompressed => Box::new(GhosttyTerm::new(config)),
        }
    }
}

/// Same generator as `xtask/src/workloads.rs` so the bytes are identical.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as u32) % n
    }
}

/// Plain printable ASCII lines ending in CRLF.
fn ascii_lines(len: usize) -> Vec<u8> {
    let mut rng = Lcg(1);
    let mut out = Vec::with_capacity(len + 256);
    while out.len() < len {
        let width = 20 + rng.below(100);
        for _ in 0..width {
            out.push(b' ' + rng.below(95) as u8);
        }
        out.extend_from_slice(b"\r\n");
    }
    out.truncate(len);
    out
}

/// Feeds `data` in default budgets and returns MB/s, best of three.
fn parser_throughput(backend: Backend, data: &[u8]) -> f64 {
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let mut term = backend.make(TermConfig {
            size: GridSize::new(120, 40),
            ..TermConfig::default()
        });
        let mut replies = Vec::new();
        let start = Instant::now();
        let mut rest = data;
        while !rest.is_empty() {
            let fed = term.feed(rest, Budget::default());
            rest = &rest[fed.consumed..];
            term.drain_replies(&mut replies);
            replies.clear();
        }
        best = best.min(start.elapsed());
    }
    data.len() as f64 / 1e6 / best.as_secs_f64()
}

/// Median full and one-line snapshot cost at 200×60, in ms.
fn snapshot_costs(backend: Backend) -> (f64, f64) {
    let mut term = backend.make(TermConfig {
        size: GridSize::new(200, 60),
        ..TermConfig::default()
    });
    let fill = ascii_lines(200 * 60 * 4);
    term.feed(&fill, Budget::UNLIMITED);
    let mut frame = Frame::default();
    term.snapshot(&mut frame);
    let (mut full, mut line) = (Vec::new(), Vec::new());
    for i in 0..200 {
        term.feed(b"\x1b[H\x1b[2J", Budget::UNLIMITED);
        term.feed(&fill[..200 * 60], Budget::UNLIMITED);
        let t = Instant::now();
        term.snapshot(&mut frame);
        full.push(t.elapsed());
        term.feed(
            format!("\x1b[30;1Hline {i:>6}").as_bytes(),
            Budget::UNLIMITED,
        );
        let t = Instant::now();
        term.snapshot(&mut frame);
        line.push(t.elapsed());
    }
    let median = |s: &mut Vec<Duration>| {
        s.sort();
        s[s.len() / 2].as_secs_f64() * 1e3
    };
    (median(&mut full), median(&mut line))
}

fn rss() -> usize {
    memory_stats::memory_stats()
        .map(|m| m.physical_mem)
        .expect("resident memory is not available on this platform")
}

/// Average RSS growth and core estimate per 80×24 tab with 10k lines of
/// history, in MiB.
fn memory_per_tab(backend: Backend) -> (f64, f64) {
    let mut history = Vec::new();
    for i in 0..10_000 {
        history.extend_from_slice(format!("{i:>8} {}\r\n", "x".repeat(71)).as_bytes());
    }
    let before = rss();
    let mut terms = Vec::with_capacity(TABS);
    for _ in 0..TABS {
        let config = TermConfig {
            size: GridSize::new(80, 24),
            scrollback_lines: 10_000,
            ..TermConfig::default()
        };
        let mut frame = Frame::default();
        let term: Box<dyn TermCore> = if backend == Backend::GhosttyCompressed {
            let mut term = GhosttyTerm::new(config);
            term.feed(&history, Budget::UNLIMITED);
            term.snapshot(&mut frame);
            assert!(term.compress_scrollback(), "compression unsupported");
            Box::new(term)
        } else {
            let mut term = backend.make(config);
            term.feed(&history, Budget::UNLIMITED);
            term.snapshot(&mut frame);
            term
        };
        // 10k lines plus the cursor line, minus the 24-row screen.
        assert!(frame.history_len >= 9_977, "history {}", frame.history_len);
        terms.push((term, frame));
    }
    let after = rss();
    let estimate: usize = terms.iter().map(|(t, _)| t.memory_estimate()).sum();
    let mib = |b: usize| b as f64 / TABS as f64 / MIB as f64;
    (mib(after.saturating_sub(before)), mib(estimate))
}

fn main() {
    let backend = match std::env::args().nth(1).as_deref() {
        Some("alacritty") => Backend::Alacritty,
        Some("ghostty") => Backend::Ghostty,
        Some("ghostty-compressed") => Backend::GhosttyCompressed,
        _ => {
            eprintln!("usage: backend_compare alacritty|ghostty|ghostty-compressed");
            std::process::exit(2);
        }
    };
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; numbers are not meaningful (use --release)");
    }
    // Memory first, before throughput runs have grown the heap.
    let (rss_mib, estimate_mib) = memory_per_tab(backend);
    println!("memory per tab (RSS) 80x24 + 10k lines   {rss_mib:>8.3} MiB");
    println!("memory per tab (core estimate)           {estimate_mib:>8.3} MiB");
    if backend == Backend::GhosttyCompressed {
        return;
    }
    let data = ascii_lines(64 * MIB);
    println!(
        "parser ascii (64 MiB, default budget)    {:>8.1} MB/s",
        parser_throughput(backend, &data)
    );
    let (full_ms, line_ms) = snapshot_costs(backend);
    println!("snapshot full 200x60                     {full_ms:>8.3} ms");
    println!("snapshot one line 200x60                 {line_ms:>8.3} ms");
}
