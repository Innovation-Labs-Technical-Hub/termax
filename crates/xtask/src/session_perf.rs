//! Measurements through `termax-session`: the per-session data path (bounded
//! queue, budgeted parser task, triple-buffered frames) under flood, and the
//! local overhead of Duplicate against a local sshd.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use termax_session::pipeline::{self, PipelineConfig, PipelineEvent};
use termax_session::{
    ClientKind, ClientRef, DuplicateMode, EventKind, OpenRequest, RuntimeConfig, SessionRuntime,
    SessionState, SystemClock,
};
use termax_sshd_fixture::{Sshd, trusted_ctx};
use termax_term::{GridSize, TermConfig, TermEvent};

use crate::perf::Factory;
use crate::workloads;

const IO_CHUNK: usize = 32 * 1024;
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const ECHO_EVERY: Duration = Duration::from_millis(20);
const FLOOD_RUN: Duration = Duration::from_secs(2);

/// Time from "remote echo sent" to "visible in a frame the UI pulled" while
/// the host floods the terminal, through the session runtime's data path:
/// the reader pushes into the window-sized queue, the parser task feeds in
/// budgets on the runtime's I/O workers, and the UI thread pulls the latest
/// frame from the triple buffer at 60 Hz.
///
/// Returns (p50, p99) in milliseconds.
pub fn flood_echo_latency(make: Factory, window_bytes: usize) -> Result<(f64, f64)> {
    let rt = SessionRuntime::new(RuntimeConfig::default())?;
    let mut pipe = pipeline::spawn(
        rt.handle(),
        make(TermConfig {
            size: GridSize::new(120, 40),
            ..TermConfig::default()
        }),
        PipelineConfig {
            window_bytes,
            ..PipelineConfig::default()
        },
        Arc::new(SystemClock),
    );
    let flood: Arc<[u8]> = workloads::ascii_lines(8 << 20).into();
    let sent_at: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let input = pipe.input.clone();
    let stamps = Arc::clone(&sent_at);
    let producer = rt.handle().spawn(async move {
        let start = Instant::now();
        let mut next_echo = start + ECHO_EVERY;
        let mut offset = 0;
        while start.elapsed() < FLOOD_RUN {
            let end = (offset + IO_CHUNK).min(flood.len());
            if input.push(flood[offset..end].to_vec()).await.is_err() {
                return;
            }
            offset = if end == flood.len() { 0 } else { end };
            if Instant::now() >= next_echo {
                let marker = {
                    let mut s = stamps.lock().unwrap();
                    let m = format!("\x1b]0;E{}\x07", s.len());
                    s.push(Instant::now());
                    m
                };
                if input.push(marker.into_bytes()).await.is_err() {
                    return;
                }
                next_echo += ECHO_EVERY;
            }
        }
    });
    drop(pipe.input);

    // Marker number → generation of the first frame that shows it.
    let mut visible_in: HashMap<usize, u64> = HashMap::new();
    let mut latencies = Vec::new();
    let mut parser_done = false;
    while !parser_done {
        std::thread::sleep(FRAME_INTERVAL);
        let generation = pipe.frames.latest().generation;
        let pulled = Instant::now();
        loop {
            match pipe.events.try_recv() {
                Ok(PipelineEvent::Term {
                    generation: g,
                    event: TermEvent::Title(t),
                }) => {
                    if let Some(n) = t.strip_prefix('E').and_then(|n| n.parse().ok()) {
                        visible_in.insert(n, g);
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    parser_done = true;
                    break;
                }
            }
        }
        let stamps = sent_at.lock().unwrap();
        visible_in.retain(|&n, &mut g| {
            if g <= generation {
                latencies.push(pulled - stamps[n]);
                false
            } else {
                true
            }
        });
    }
    rt.block_on(producer).context("flood producer")?;
    if latencies.is_empty() {
        bail!("no echo marker became visible");
    }
    latencies.sort();
    let pct = |p: f64| {
        let i = ((latencies.len() as f64 * p).ceil() as usize).clamp(1, latencies.len()) - 1;
        latencies[i].as_secs_f64() * 1e3
    };
    Ok((pct(0.50), pct(0.99)))
}

/// Median local overhead of Duplicate on an open connection (time in
/// Termax code, excluding the network wait for the channel).
pub fn duplicate_overhead_ms(sshd: &Sshd) -> Result<f64> {
    let rt = SessionRuntime::new(RuntimeConfig::default())?;
    let mut rx = rt.subscribe();
    let owner = ClientRef::new(ClientKind::Ui, "perf");
    let src = rt.open(OpenRequest::new(sshd.opts(), trusted_ctx(sshd), owner));
    let mut samples = Vec::new();
    rt.block_on(async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.recv())
                .await
                .context("source session did not open")??;
            if ev.session == src.id()
                && matches!(
                    &ev.kind,
                    EventKind::StateChanged {
                        to: SessionState::Open,
                        ..
                    }
                )
            {
                break;
            }
        }
        for _ in 0..9 {
            let dup = rt.duplicate(src.id(), DuplicateMode::SameConnection)?;
            loop {
                let ev = tokio::time::timeout_at(deadline, rx.recv())
                    .await
                    .context("duplicate did not open")??;
                if ev.session != dup.id() {
                    continue;
                }
                match ev.kind {
                    EventKind::Duplicated {
                        local_overhead: Some(d),
                        ..
                    } => {
                        samples.push(d);
                        break;
                    }
                    EventKind::Duplicated { .. } => bail!("duplicate did not reuse the connection"),
                    EventKind::StateChanged {
                        to: SessionState::Disconnected(d),
                        ..
                    } => bail!("duplicate failed: {d}"),
                    _ => {}
                }
            }
            dup.close();
        }
        anyhow::Ok(())
    })?;
    samples.sort();
    Ok(samples[samples.len() / 2].as_secs_f64() * 1e3)
}
