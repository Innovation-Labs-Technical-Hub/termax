//! The per-session terminal data path.
//!
//! ```text
//! channel reader ──push──▶ bounded queue (credit = window size) ──▶ parser task
//!                                                                   │ TermCore::feed(budget)
//!        ◀── credit for consumed bytes ─────────────────────────────┤
//!        ◀── replies (DA, DSR, …) to the host ──────────────────────┤
//!   UI ◀── latest Frame (triple buffer) ◀── snapshot ───────────────┤
//!      ◀── events (title, cwd, bell, compaction) ───────────────────┘
//! ```
//!
//! The queue holds at most `window_bytes` of unparsed output: a push waits
//! until the parser has consumed (and so credited back) enough bytes. Each
//! `feed` call is bounded by a [`Budget`], and the task yields between
//! budgets so parsing never monopolises an I/O worker. After output has been
//! quiet for the configured period the task calls [`TermCore::on_idle`] once.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use termax_term::{Budget, Frame, TermCore, TermEvent};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;

use crate::clock::Clock;

/// Default parsed-output queue, equal to the default SSH window (2 MiB).
pub const DEFAULT_WINDOW_BYTES: usize = termax_ssh::MIN_WINDOW_SIZE as usize;
/// Largest accepted queue (4 MiB): beyond this, echo under flood gets slow.
pub const MAX_WINDOW_BYTES: usize = 4 * 1024 * 1024;
/// While output keeps arriving, a frame is published at least this often.
pub const DEFAULT_PUBLISH_INTERVAL: Duration = Duration::from_millis(8);
/// Default quiet period before idle compaction.
///
/// [TO CONFIRM · CHỜ XÁC NHẬN] Provisional value, owner: Tech lead. It is a
/// setting (`RuntimeConfig::idle_compaction_quiet_period`); only the default
/// awaits a decision.
pub const DEFAULT_IDLE_COMPACTION_QUIET_PERIOD: Duration = Duration::from_secs(60);

/// Data path settings.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Queue capacity in bytes (the SSH window).
    pub window_bytes: usize,
    /// Budget per `feed` call.
    pub budget: Budget,
    /// Longest gap between published frames while output flows.
    pub publish_interval: Duration,
    /// Quiet time before `on_idle`.
    pub idle_quiet_period: Duration,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            window_bytes: DEFAULT_WINDOW_BYTES,
            budget: Budget::default(),
            publish_interval: DEFAULT_PUBLISH_INTERVAL,
            idle_quiet_period: DEFAULT_IDLE_COMPACTION_QUIET_PERIOD,
        }
    }
}

/// The terminal core, shared between the parser task and the UI. The parser
/// holds the lock for one budget at a time (≤ 2 ms by default).
pub type SharedTerm = Arc<Mutex<Box<dyn TermCore>>>;

/// Lock a shared terminal, recovering from a poisoned lock (a panic in a
/// backend must not take the whole session down).
pub fn lock_term(term: &SharedTerm) -> MutexGuard<'_, Box<dyn TermCore>> {
    term.lock().unwrap_or_else(|p| p.into_inner())
}

/// What the parser reports besides frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineEvent {
    /// A terminal event, visible from frame `generation` on.
    Term {
        /// Frame generation that includes the event's effects.
        generation: u64,
        /// The event.
        event: TermEvent,
    },
    /// `on_idle` ran after the quiet period.
    Compacted,
}

/// The queue is closed (the parser task stopped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("terminal pipeline closed")]
pub struct PipelineClosed;

/// Writer side of the bounded queue. Cloneable; the parser task ends once
/// every clone is dropped and the queue is drained.
#[derive(Clone)]
pub struct PipelineInput {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    credit: Arc<Semaphore>,
    window: usize,
}

impl PipelineInput {
    /// Queue `bytes`, waiting while the queue is full. Back-pressure starts
    /// here: a caller that awaits this stops reading the channel.
    pub async fn push(&self, bytes: Vec<u8>) -> Result<(), PipelineClosed> {
        if bytes.len() <= self.window {
            return self.push_piece(bytes).await;
        }
        for piece in bytes.chunks(self.window) {
            self.push_piece(piece.to_vec()).await?;
        }
        Ok(())
    }

    async fn push_piece(&self, bytes: Vec<u8>) -> Result<(), PipelineClosed> {
        if bytes.is_empty() {
            return Ok(());
        }
        // `window` ≤ 4 MiB, so a piece always fits in u32.
        let n = u32::try_from(bytes.len()).map_err(|_| PipelineClosed)?;
        let permit = self
            .credit
            .acquire_many(n)
            .await
            .map_err(|_| PipelineClosed)?;
        permit.forget();
        self.tx.send(bytes).map_err(|_| PipelineClosed)
    }

    /// Bytes queued and not yet consumed by the parser.
    pub fn queued_bytes(&self) -> usize {
        self.window.saturating_sub(self.credit.available_permits())
    }

    /// Queue capacity.
    pub fn window_bytes(&self) -> usize {
        self.window
    }
}

#[derive(Default)]
struct Slot {
    frame: Frame,
    fresh: bool,
}

/// Reader side of the frame triple buffer: the UI calls
/// [`FrameReader::latest`] at vsync and never waits for the parser.
pub struct FrameReader {
    shared: Arc<Mutex<Slot>>,
    front: Frame,
}

impl FrameReader {
    /// Swap in the newest published frame, if any, and return it.
    pub fn latest(&mut self) -> &Frame {
        let mut slot = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        if slot.fresh {
            std::mem::swap(&mut self.front, &mut slot.frame);
            slot.fresh = false;
        }
        drop(slot);
        &self.front
    }

    /// Whether a frame newer than the last [`FrameReader::latest`] is waiting.
    pub fn has_new(&self) -> bool {
        self.shared.lock().unwrap_or_else(|p| p.into_inner()).fresh
    }
}

struct FramePublisher {
    shared: Arc<Mutex<Slot>>,
    back: Frame,
}

impl FramePublisher {
    fn publish(&mut self, frame: &Frame) {
        self.back.clone_from(frame);
        let mut slot = self.shared.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::swap(&mut slot.frame, &mut self.back);
        slot.fresh = true;
    }
}

/// Everything [`spawn`] hands back.
pub struct Pipeline {
    /// Push channel output here.
    pub input: PipelineInput,
    /// Latest frames for the renderer.
    pub frames: FrameReader,
    /// Terminal events and compaction notices.
    pub events: mpsc::UnboundedReceiver<PipelineEvent>,
    /// Bytes the terminal must send back to the host.
    pub replies: mpsc::UnboundedReceiver<Vec<u8>>,
    /// The terminal core (for input encoding, scrolling, selection, search).
    pub term: SharedTerm,
    /// The parser task; it ends when every [`PipelineInput`] is dropped.
    pub task: JoinHandle<()>,
}

/// Start a parser task for `term` on `rt`.
pub fn spawn(
    rt: &tokio::runtime::Handle,
    term: Box<dyn TermCore>,
    config: PipelineConfig,
    clock: Arc<dyn Clock>,
) -> Pipeline {
    let window = config.window_bytes.clamp(1, MAX_WINDOW_BYTES);
    let credit = Arc::new(Semaphore::new(window));
    let (tx, rx) = mpsc::unbounded_channel();
    let (ev_tx, events) = mpsc::unbounded_channel();
    let (reply_tx, replies) = mpsc::unbounded_channel();
    let shared = Arc::new(Mutex::new(Slot::default()));
    let term: SharedTerm = Arc::new(Mutex::new(term));
    let parser = Parser {
        term: Arc::clone(&term),
        rx,
        credit: Arc::clone(&credit),
        events: ev_tx,
        replies: reply_tx,
        publisher: FramePublisher {
            shared: Arc::clone(&shared),
            back: Frame::default(),
        },
        working: Frame::default(),
        config,
        clock,
    };
    let task = rt.spawn(parser.run());
    Pipeline {
        input: PipelineInput { tx, credit, window },
        frames: FrameReader {
            shared,
            front: Frame::default(),
        },
        events,
        replies,
        term,
        task,
    }
}

struct Parser {
    term: SharedTerm,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    credit: Arc<Semaphore>,
    events: mpsc::UnboundedSender<PipelineEvent>,
    replies: mpsc::UnboundedSender<Vec<u8>>,
    publisher: FramePublisher,
    working: Frame,
    config: PipelineConfig,
    clock: Arc<dyn Clock>,
}

impl Parser {
    async fn run(mut self) {
        let mut queue: VecDeque<Vec<u8>> = VecDeque::new();
        let mut offset = 0usize;
        let mut pending_events = Vec::new();
        let mut reply_buf = Vec::new();
        let mut dirty = false;
        let mut last_publish = self.clock.now();
        // When output last arrived; `None` once compacted (or before any).
        let mut quiet_since: Option<Instant> = None;

        loop {
            while let Ok(chunk) = self.rx.try_recv() {
                queue.push_back(chunk);
            }
            if queue.is_empty() {
                if dirty {
                    self.publish(&mut pending_events);
                    dirty = false;
                    last_publish = self.clock.now();
                }
                let idle_at = quiet_since.map(|t| t + self.config.idle_quiet_period);
                let idle = async {
                    match idle_at {
                        Some(at) => self.clock.sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    chunk = self.rx.recv() => match chunk {
                        Some(c) => queue.push_back(c),
                        None => return,
                    },
                    () = idle => {
                        lock_term(&self.term).on_idle();
                        quiet_since = None;
                        let _ = self.events.send(PipelineEvent::Compacted);
                    }
                }
                continue;
            }

            let Some(front) = queue.front() else { continue };
            let consumed = {
                let mut term = lock_term(&self.term);
                let fed = term.feed(&front[offset..], self.config.budget);
                term.drain_replies(&mut reply_buf);
                term.take_events(&mut pending_events);
                fed.consumed
            };
            if !reply_buf.is_empty() {
                let _ = self.replies.send(std::mem::take(&mut reply_buf));
            }
            offset += consumed;
            if offset >= front.len() {
                queue.pop_front();
                offset = 0;
            }
            self.credit.add_permits(consumed);
            if consumed > 0 {
                dirty = true;
                quiet_since = Some(self.clock.now());
            }
            if dirty && self.clock.now() - last_publish >= self.config.publish_interval {
                self.publish(&mut pending_events);
                dirty = false;
                last_publish = self.clock.now();
            }
            tokio::task::yield_now().await;
        }
    }

    /// Snapshot into the working frame (incremental), hand a copy to the
    /// triple buffer, then release the events that frame includes.
    fn publish(&mut self, pending: &mut Vec<TermEvent>) {
        {
            let mut term = lock_term(&self.term);
            term.snapshot(&mut self.working);
            term.take_events(pending);
        }
        self.publisher.publish(&self.working);
        let generation = self.working.generation;
        for event in pending.drain(..) {
            let _ = self.events.send(PipelineEvent::Term { generation, event });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use termax_term::{AlacrittyTerm, GridSize, TermConfig};

    fn small_term() -> Box<dyn TermCore> {
        Box::new(AlacrittyTerm::new(TermConfig {
            size: GridSize::new(40, 5),
            ..TermConfig::default()
        }))
    }

    fn rows_text(frame: &Frame) -> Vec<String> {
        (0..frame.rows.len())
            .map(|i| frame.row_text(i, |_| None).trim_end().to_owned())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn output_is_parsed_published_and_replies_returned() {
        let clock: Arc<dyn Clock> = Arc::new(crate::clock::SystemClock);
        let mut p = spawn(
            &tokio::runtime::Handle::current(),
            small_term(),
            PipelineConfig::default(),
            clock,
        );
        // Text, a title and a primary device attributes query.
        p.input
            .push(b"hello\r\n\x1b]0;tab\x07\x1b[c".to_vec())
            .await
            .unwrap();
        let reply = p.replies.recv().await.unwrap();
        assert!(reply.starts_with(b"\x1b[?"), "{reply:?}");
        let ev = p.events.recv().await.unwrap();
        let PipelineEvent::Term { generation, event } = ev else {
            panic!("{ev:?}")
        };
        assert_eq!(event, TermEvent::Title("tab".into()));
        assert!(generation >= 1);
        let frame = p.frames.latest();
        assert!(frame.generation >= generation);
        assert_eq!(rows_text(frame)[0], "hello");
        assert_eq!(p.input.queued_bytes(), 0, "consumed bytes credited back");
        drop(p.input);
        p.task.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queue_never_holds_more_than_the_window() {
        let clock: Arc<dyn Clock> = Arc::new(crate::clock::SystemClock);
        let window = 64 * 1024;
        let p = spawn(
            &tokio::runtime::Handle::current(),
            small_term(),
            PipelineConfig {
                window_bytes: window,
                budget: Budget {
                    max_bytes: 1024,
                    max_time: Duration::from_millis(2),
                },
                ..PipelineConfig::default()
            },
            clock,
        );
        let input = p.input.clone();
        let producer = tokio::spawn(async move {
            for _ in 0..64 {
                input.push(vec![b'x'; 16 * 1024]).await.unwrap();
                assert!(input.queued_bytes() <= window);
            }
        });
        let mut max_seen = 0;
        while !producer.is_finished() {
            max_seen = max_seen.max(p.input.queued_bytes());
            tokio::task::yield_now().await;
        }
        producer.await.unwrap();
        assert!(max_seen <= window, "{max_seen} > {window}");
        // An oversized push is split rather than deadlocking.
        p.input.push(vec![b'y'; 3 * window]).await.unwrap();
    }

    fn history(lines: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..lines {
            out.extend_from_slice(format!("{i:>8} {}\r\n", "x".repeat(60)).as_bytes());
        }
        out.extend_from_slice(b"user@host:~$ ");
        out
    }

    /// Wait until the parser has drained its queue and is idle-waiting.
    async fn settle(p: &Pipeline, clock: &ManualClock, sleepers: usize) {
        while p.input.queued_bytes() > 0 || clock.sleepers() < sleepers {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Scenario: FR-CO01-011 busy sessions are not compacted
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn busy_sessions_are_not_compacted() {
        let clock = Arc::new(ManualClock::new());
        let quiet = Duration::from_secs(60);
        let mut p = spawn(
            &tokio::runtime::Handle::current(),
            crate::default_term(TermConfig::default()),
            PipelineConfig {
                idle_quiet_period: quiet,
                ..PipelineConfig::default()
            },
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        // Output every 40 s for 10 quiet periods' worth of time.
        for i in 0..15 {
            p.input
                .push(format!("line {i}\r\n").into_bytes())
                .await
                .unwrap();
            settle(&p, &clock, 1).await;
            clock.advance(Duration::from_secs(40));
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        while let Ok(ev) = p.events.try_recv() {
            assert_ne!(ev, PipelineEvent::Compacted, "busy session compacted");
        }
        // Once it goes quiet, it is compacted exactly once.
        clock.advance(quiet);
        assert_eq!(next_compaction(&mut p).await, PipelineEvent::Compacted);
        clock.advance(quiet * 3);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            p.events.try_recv().is_err(),
            "compacted only once per quiet period"
        );
    }

    async fn next_compaction(p: &mut Pipeline) -> PipelineEvent {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), p.events.recv())
                .await
                .expect("compaction")
                .expect("pipeline alive");
            if ev == PipelineEvent::Compacted {
                return ev;
            }
        }
    }

    /// Scenario: FR-CO01-011 compaction is invisible to the user
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compaction_is_invisible_to_the_user() {
        use termax_term::{SearchQuery, SelectionKind, SelectionOp, ViewportPoint, ViewportScroll};
        let clock = Arc::new(ManualClock::new());
        let mut p = spawn(
            &tokio::runtime::Handle::current(),
            crate::default_term(TermConfig {
                size: GridSize::new(80, 24),
                ..TermConfig::default()
            }),
            PipelineConfig::default(),
            Arc::clone(&clock) as Arc<dyn Clock>,
        );
        p.input.push(history(10_000)).await.unwrap();
        settle(&p, &clock, 1).await;

        let observe = |p: &Pipeline| {
            let mut term = lock_term(&p.term);
            term.scroll(ViewportScroll::Up(5_000));
            let mut frame = Frame::default();
            term.snapshot(&mut frame);
            let rows = rows_text(&frame);
            term.scroll(ViewportScroll::Bottom);
            let copied = term.selection_text();
            let matches = term
                .search(&SearchQuery::new(" 4242 xxx"))
                .expect("valid regex");
            (rows, copied, matches)
        };
        {
            // Select a few history lines while scrolled back, then return.
            let mut term = lock_term(&p.term);
            term.scroll(ViewportScroll::Up(5_000));
            term.select(SelectionOp::Start {
                at: ViewportPoint::new(2, 0),
                kind: SelectionKind::Char,
            });
            term.select(SelectionOp::Extend {
                to: ViewportPoint::new(5, 30),
            });
            term.scroll(ViewportScroll::Bottom);
        }
        let before = observe(&p);
        assert!(before.1.as_deref().is_some_and(|t| t.contains("x")));
        assert_eq!(before.2.len(), 1);

        clock.advance(DEFAULT_IDLE_COMPACTION_QUIET_PERIOD);
        assert_eq!(next_compaction(&mut p).await, PipelineEvent::Compacted);
        assert_eq!(
            observe(&p),
            before,
            "rows, copied text and matches unchanged"
        );
    }
}
