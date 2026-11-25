//! [`TermCore`] backend on Ghostty's terminal core (libghostty-vt).
//!
//! Building needs Zig 0.15.2 on `PATH`; the first build clones Ghostty at the
//! commit pinned by `libghostty-vt-sys`.
//!
//! Ghostty parses the stream and owns the grid. The rest of the trait is
//! filled in around it so this backend behaves like the alacritty one:
//!
//! * The shared [`OscScanner`] reports OSC 7, OSC 133 prompt marks and OSC 52
//!   clipboard reads (which Ghostty drops itself); every byte still reaches
//!   Ghostty.
//! * Selection and search read cells through grid references and apply
//!   alacritty's rules (see the `selection` and `search` modules). Selection
//!   endpoints are Ghostty tracked references, so they follow their content
//!   through output, scrolling and reflow.
//! * [`TermCore::on_idle`] runs Ghostty's scrollback compression.
//!
//! Remaining differences: `CursorStyle` events are raised at
//! [`TermCore::snapshot`] time rather than while feeding, because Ghostty only
//! exposes the cursor shape through its render state; an empty OSC 0/2 title
//! is reported as [`TermEvent::ResetTitle`] (alacritty reports `Title("")`).

mod csi_scan;
mod grid;
mod search;
mod selection;

use std::collections::{HashMap, VecDeque};
use std::mem;
use std::time::Instant;

use termax_term::input::{self, InputModes, KeyEvent, MouseEvent, PasteOutcome, PastePolicy};
use termax_term::osc_scan::{ClipboardLoad, OscEvent, OscScanner, ScanAction};
use termax_term::{
    Budget, Cell16, ClipboardLoadMode, ClipboardStoreMode, ClipboardTarget, CursorShape,
    CursorState, Damage, Fed, Frame, GRAPHEME_BIT, GridSize, Match, MouseEncoding, MouseTracking,
    Palette, RowSet, RowView, SearchError, SearchQuery, SelectionKind, SelectionOp, SelectionSpan,
    TermConfig, TermCore, TermEvent, ViewportPoint, ViewportScroll, cell_flags,
};
use termax_term_ghostty_sys::vt::error::{Error as VtError, Result as VtResult};
use termax_term_ghostty_sys::vt::render::{CellIteration, CursorVisualStyle, Dirty, RowIteration};
use termax_term_ghostty_sys::vt::screen::{CellContentTag, CellWide};
use termax_term_ghostty_sys::vt::style::{self, RgbColor, StyleColor, Underline};
use termax_term_ghostty_sys::vt::terminal::{
    ClipboardLocation, CompressionMode, CompressionResult, ConformanceLevel, DeviceAttributes,
    DeviceType, Mode, Point, PointCoordinate, PointSpace, PrimaryDeviceAttributes, ScrollViewport,
    SecondaryDeviceAttributes, TertiaryDeviceAttributes,
};
use termax_term_ghostty_sys::{Effect, Vt, VtOptions};

use crate::csi_scan::{CsiOp, CsiScanner};
use crate::grid::{Grid, Pos};

/// Input is handed to the parser in chunks of this size; the time budget is
/// checked between chunks.
const FEED_CHUNK: usize = 16 * 1024;

/// Grapheme table size at which it is reset (forcing a full repaint).
const MAX_GRAPHEMES: usize = 65_536;

/// Bytes Ghostty stores per cell (`page.Cell` is a packed `u64`).
const GHOSTTY_CELL_BYTES: usize = 8;

/// Bytes Ghostty stores per row header (`page.Row` is a packed `u64`).
const GHOSTTY_ROW_BYTES: usize = 8;

/// Unanswered clipboard-load requests kept before the oldest is dropped.
const MAX_PENDING_LOADS: usize = 16;

/// Saved titles kept before the oldest is dropped (alacritty's limit).
const MAX_TITLE_STACK: usize = 4096;

/// Tracked-reference slots ([`Vt::set_mark`]) used by the selection.
const MARK_ANCHOR: usize = 0;
const MARK_START: usize = 1;
const MARK_END: usize = 2;
const MARK_COUNT: usize = 3;

/// Error creating a [`GhosttyTerm`].
pub type Error = VtError;

/// Interns multi-codepoint graphemes so cells stay 16 bytes.
#[derive(Default)]
struct GraphemeTable {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl GraphemeTable {
    fn intern(&mut self, text: &str) -> u32 {
        if let Some(&id) = self.index.get(text) {
            return GRAPHEME_BIT | id;
        }
        // Bounded by MAX_GRAPHEMES plus one screen, far below 2^31.
        let id = self.strings.len() as u32;
        self.strings.push(text.to_owned());
        self.index.insert(text.to_owned(), id);
        GRAPHEME_BIT | id
    }

    fn get(&self, glyph: u32) -> Option<&str> {
        if glyph & GRAPHEME_BIT == 0 {
            return None;
        }
        self.strings
            .get((glyph & !GRAPHEME_BIT) as usize)
            .map(String::as_str)
    }

    fn bytes(&self) -> usize {
        self.strings
            .iter()
            .map(|s| 2 * s.len() + 2 * mem::size_of::<String>() + 8)
            .sum()
    }
}

/// Per-snapshot conversion state shared by the render callbacks.
struct RenderCtx<'a> {
    frame: &'a mut Frame,
    damage: Damage,
    graphemes: &'a mut GraphemeTable,
    force_full: bool,
    default_fg: u32,
    default_bg: u32,
    palette: [RgbColor; 256],
    cursor: CursorState,
    chars: Vec<char>,
    text: String,
}

/// The current selection. Its cells live in Ghostty tracked references:
/// [`MARK_ANCHOR`] is where it started, [`MARK_START`]/[`MARK_END`] bound the
/// resolved range.
struct Selection {
    kind: SelectionKind,
    /// The alternate screen was active when it started.
    alt: bool,
    /// `None` while empty (a char or block selection not yet extended).
    range: Option<ResolvedRange>,
}

#[derive(Clone, Copy)]
struct ResolvedRange {
    block: bool,
    /// Start column, kept to clamp a block whose first row left the scrollback.
    start_col: u16,
}

/// A terminal emulator instance backed by Ghostty's libghostty-vt.
pub struct GhosttyTerm {
    vt: Vt,
    scanner: OscScanner,
    actions: Vec<ScanAction>,
    csi: CsiScanner,
    csi_ops: Vec<(usize, CsiOp)>,
    replies: Vec<u8>,
    events: Vec<TermEvent>,
    config: TermConfig,
    graphemes: GraphemeTable,
    force_full: bool,
    size: GridSize,
    cursor_style: (CursorShape, bool),
    selection: Option<Selection>,
    /// Current title as reported to the embedder; `None` when unset.
    title: Option<String>,
    /// Titles saved by XTWINOPS 22, restored by 23.
    title_stack: Vec<Option<String>>,
    /// Column of each mark when it was set; see [`GhosttyTerm::mark`].
    mark_cols: [u16; MARK_COUNT],
    next_load_id: u64,
    pending_loads: VecDeque<(u64, ClipboardLoad)>,
}

impl GhosttyTerm {
    /// Creates a terminal with the given configuration.
    ///
    /// # Panics
    ///
    /// Only if Ghostty cannot allocate the terminal; use [`Self::try_new`] to
    /// handle that.
    #[allow(
        clippy::expect_used,
        reason = "documented panic on allocation failure, like `Box::new`; `try_new` is the fallible form"
    )]
    pub fn new(config: TermConfig) -> Self {
        Self::try_new(config).expect("libghostty-vt failed to allocate a terminal")
    }

    /// Creates a terminal, reporting allocation failures.
    pub fn try_new(config: TermConfig) -> Result<Self, Error> {
        let size = clamp_size(config.size);
        let mut vt = Vt::new(VtOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback_bytes: scrollback_bytes(config.scrollback_lines, size.cols),
            device_attributes: device_attributes(),
        })?;
        vt.resize(size.cols, size.rows, size.cell_width, size.cell_height)?;
        let (fg, bg, palette) = ghostty_colors(&config.palette);
        vt.set_colors(fg, bg, palette)?;
        Ok(Self {
            vt,
            scanner: OscScanner::default().with_clipboard_loads(),
            actions: Vec::new(),
            csi: CsiScanner::default(),
            csi_ops: Vec::new(),
            replies: Vec::new(),
            events: Vec::new(),
            config: TermConfig { size, ..config },
            graphemes: GraphemeTable::default(),
            force_full: true,
            size,
            cursor_style: (CursorShape::Block, false),
            selection: None,
            title: None,
            title_stack: Vec::new(),
            mark_cols: [0; MARK_COUNT],
            next_load_id: 1,
            pending_loads: VecDeque::new(),
        })
    }

    /// Runs Ghostty's scrollback compression to completion; [`TermCore::on_idle`]
    /// calls this. Returns `false` where the platform cannot reclaim the memory
    /// (Ghostty reports compression as unsupported there) or on failure.
    pub fn compress_scrollback(&mut self) -> bool {
        matches!(
            self.vt.compress(CompressionMode::Full),
            Ok(CompressionResult::Complete | CompressionResult::Pending)
        )
    }

    /// Splits `chunk` around the OSC sequences the scanner reports, so each
    /// event sees the emulator state after the bytes that preceded it.
    fn process_chunk(&mut self, chunk: &[u8]) {
        let mut actions = mem::take(&mut self.actions);
        actions.clear();
        self.scanner.scan(chunk, &mut actions);
        for action in actions.drain(..) {
            match action {
                ScanAction::Forward(range) => self.write(&chunk[range]),
                ScanAction::Inject(bytes) => self.write(bytes),
                ScanAction::Event(event) => {
                    // Keep emulator events that preceded the OSC in order.
                    self.drain_effects();
                    self.push_osc_event(event);
                }
            }
        }
        self.actions = actions;
        self.drain_effects();
        if self
            .selection
            .as_ref()
            .is_some_and(|sel| sel.alt != self.vt.alt_screen().unwrap_or(false))
        {
            // alacritty drops the selection whenever the screens swap.
            self.clear_selection();
        }
    }

    /// Hands bytes to Ghostty, applying the [`CsiOp`]s found in them right
    /// after Ghostty has processed each one.
    fn write(&mut self, bytes: &[u8]) {
        let mut ops = mem::take(&mut self.csi_ops);
        ops.clear();
        self.csi.scan(bytes, &mut ops);
        let mut start = 0;
        for &(end, op) in &ops {
            self.vt.write(&bytes[start..end]);
            start = end;
            self.after_csi(op);
        }
        self.vt.write(&bytes[start..]);
        self.csi_ops = ops;
    }

    fn after_csi(&mut self, op: CsiOp) {
        match op {
            CsiOp::PushTitle => {
                // Pick up title changes that preceded the push.
                self.drain_effects();
                if self.title_stack.len() >= MAX_TITLE_STACK {
                    self.title_stack.remove(0);
                }
                self.title_stack.push(self.title.clone());
            }
            CsiOp::PopTitle => {
                self.drain_effects();
                if let Some(title) = self.title_stack.pop() {
                    self.events.push(match &title {
                        Some(title) => TermEvent::Title(title.clone()),
                        None => TermEvent::ResetTitle,
                    });
                    self.title = title;
                }
            }
            CsiOp::Reset => {
                // Like alacritty's reset: forget the titles without an event.
                self.drain_effects();
                self.title = None;
                self.title_stack.clear();
                self.clear_selection();
            }
            CsiOp::EraseDisplay(_) | CsiOp::EraseLine(_) => self.after_erase(op),
        }
    }

    /// alacritty's rule: ED 2 always clears the selection; ED 0/1/3 and EL
    /// clear it when it touches the erased lines.
    fn after_erase(&mut self, op: CsiOp) {
        let Some((start, end, _)) = self.selection_range() else {
            return;
        };
        let history = self.history_rows();
        let rows = i64::from(self.size.rows);
        let cursor = i64::from(self.vt.cursor_y().unwrap_or(0));
        let line = |p: Pos| i64::from(p.y) - history;
        let (first, last) = (line(start), line(end));
        let touches = |top: i64, bottom: i64| first <= bottom && last >= top;
        let clear = match op {
            CsiOp::EraseDisplay(0) => touches(cursor, rows - 1),
            CsiOp::EraseDisplay(1) => touches(0, cursor),
            CsiOp::EraseDisplay(2) => true,
            CsiOp::EraseDisplay(_) => first < 0,
            CsiOp::EraseLine(_) => touches(cursor, cursor),
            CsiOp::Reset | CsiOp::PushTitle | CsiOp::PopTitle => false,
        };
        if clear {
            self.clear_selection();
        }
    }

    fn push_osc_event(&mut self, event: OscEvent) {
        let event = match event {
            OscEvent::Cwd { host, path } => TermEvent::Cwd { host, path },
            OscEvent::Prompt(kind) => TermEvent::PromptMark {
                kind,
                cursor_line: self.vt.cursor_y().unwrap_or(0),
            },
            OscEvent::ClipboardLoad(load) => {
                if self.config.clipboard.load == ClipboardLoadMode::Deny {
                    return;
                }
                let id = self.next_load_id;
                self.next_load_id += 1;
                if self.pending_loads.len() >= MAX_PENDING_LOADS {
                    self.pending_loads.pop_front();
                }
                self.pending_loads.push_back((id, load));
                TermEvent::ClipboardLoadRequest {
                    id,
                    target: load.target,
                }
            }
        };
        self.events.push(event);
    }

    fn drain_effects(&mut self) {
        let mut fx = self.vt.effects();
        self.replies.append(&mut fx.pty);
        for effect in fx.events.drain(..) {
            match effect {
                Effect::Bell => self.events.push(TermEvent::Bell),
                Effect::Title(title) => {
                    // vte trims titles; an empty one means "back to default".
                    let title = title.trim();
                    self.title = (!title.is_empty()).then(|| title.to_owned());
                    self.events.push(match &self.title {
                        Some(title) => TermEvent::Title(title.clone()),
                        None => TermEvent::ResetTitle,
                    });
                }
                Effect::ClipboardWrite { location, text } => {
                    if text.len() > self.config.clipboard.max_store_bytes {
                        continue;
                    }
                    let target = match location {
                        ClipboardLocation::Standard => ClipboardTarget::Clipboard,
                        ClipboardLocation::Selection | ClipboardLocation::Primary => {
                            ClipboardTarget::Selection
                        }
                    };
                    match self.config.clipboard.store {
                        ClipboardStoreMode::Deny => {}
                        ClipboardStoreMode::Allow => {
                            self.events.push(TermEvent::ClipboardStore { target, text });
                        }
                        ClipboardStoreMode::Ask => {
                            self.events
                                .push(TermEvent::ClipboardStoreRequest { target, text });
                        }
                    }
                }
            }
        }
    }

    fn mode(&self, mode: Mode) -> bool {
        self.vt.mode(mode).unwrap_or(false)
    }

    fn history_rows(&self) -> i64 {
        self.vt
            .scrollback_rows()
            .map_or(0, |rows| i64::try_from(rows).unwrap_or(i64::MAX))
    }

    /// Viewport cell (clamped to the grid) as a screen position.
    fn viewport_to_screen(&self, p: ViewportPoint) -> Option<Pos> {
        let point = Point::Viewport(PointCoordinate {
            x: p.col.min(self.size.cols - 1),
            y: u32::from(p.row.min(self.size.rows - 1)),
        });
        let c = self.vt.convert(point, PointSpace::Screen).ok()??;
        Some(Pos::new(c.y, c.x))
    }

    /// Where a mark's cell is now; `None` once that content is gone.
    ///
    /// Ghostty invalidates a mark whose row is pruned from the scrollback,
    /// but on a screen without scrollback (the alternate screen, or
    /// `scrollback_lines: 0`) a row that scrolls off the top is reused and
    /// its marks jump to column 0 of the top row. Marks otherwise keep their
    /// column (a width change drops the selection first), so a mark found in
    /// another column has lost its cell.
    fn mark(&self, mark: usize) -> Option<Pos> {
        let c = self.vt.mark(mark, PointSpace::Screen).ok()??;
        (self.mark_cols.get(mark) == Some(&c.x)).then(|| Pos::new(c.y, c.x))
    }

    fn set_mark(&mut self, mark: usize, p: Pos) -> bool {
        let point = Point::Screen(PointCoordinate { x: p.x, y: p.y });
        let Some(col) = self.mark_cols.get_mut(mark) else {
            return false;
        };
        *col = p.x;
        self.vt.set_mark(mark, point).is_ok()
    }

    fn clear_selection(&mut self) {
        if self.selection.take().is_some() {
            self.vt.clear_marks();
            self.force_full = true;
        }
    }

    /// Resolves the selection anchored at [`MARK_ANCHOR`] and stores its
    /// bounds. The range is fixed when the selection is made or extended.
    /// alacritty re-expands word and line selections against the current
    /// content each time it reads them; doing that here would walk the grid
    /// through grid references on every snapshot.
    fn resolve_selection(&mut self, anchor: Pos, target: Option<Pos>) {
        let Some(kind) = self.selection.as_ref().map(|sel| sel.kind) else {
            return;
        };
        let range = Grid::new(&self.vt)
            .and_then(|mut grid| selection::resolve(&mut grid, kind, anchor, target));
        let resolved =
            range.filter(|r| self.set_mark(MARK_START, r.start) && self.set_mark(MARK_END, r.end));
        if let Some(sel) = self.selection.as_mut() {
            sel.range = resolved.map(|r| ResolvedRange {
                block: r.block,
                start_col: r.start.x,
            });
        }
    }

    /// Current selection bounds in screen coordinates. Like alacritty, a start
    /// whose content is gone is clamped to the top row (keeping a block's
    /// column), and an end whose content is gone ends the selection.
    fn selection_range(&self) -> Option<(Pos, Pos, bool)> {
        let range = self.selection.as_ref()?.range?;
        let end = self.mark(MARK_END)?;
        let start = self
            .mark(MARK_START)
            .unwrap_or(Pos::new(0, if range.block { range.start_col } else { 0 }));
        Some((start, end, range.block))
    }

    /// The selection clipped to the viewport, like the alacritty backend.
    fn selection_span(&self) -> Option<SelectionSpan> {
        let (start, end, block) = self.selection_range()?;
        let top = i64::try_from(self.vt.scrollbar().ok()?.offset).ok()?;
        let rows = i64::from(self.size.rows);
        let last_col = self.size.cols - 1;
        let start_row = i64::from(start.y) - top;
        let end_row = i64::from(end.y) - top;
        if end_row < 0 || start_row >= rows {
            return None;
        }
        let start = if start_row < 0 {
            ViewportPoint::new(0, if block { start.x } else { 0 })
        } else {
            ViewportPoint::new(start_row as u16, start.x)
        };
        let end = if end_row >= rows {
            ViewportPoint::new((rows - 1) as u16, if block { end.x } else { last_col })
        } else {
            ViewportPoint::new(end_row as u16, end.x)
        };
        Some(SelectionSpan { start, end, block })
    }
}

impl TermCore for GhosttyTerm {
    fn feed(&mut self, bytes: &[u8], budget: Budget) -> Fed {
        let limit = bytes.len().min(budget.max_bytes);
        let started = Instant::now();
        let mut consumed = 0;
        while consumed < limit {
            let end = (consumed + FEED_CHUNK).min(limit);
            self.process_chunk(&bytes[consumed..end]);
            consumed = end;
            if started.elapsed() >= budget.max_time {
                break;
            }
        }
        Fed { consumed }
    }

    fn drain_replies(&mut self, out: &mut Vec<u8>) {
        out.append(&mut self.replies);
    }

    fn input_modes(&self) -> InputModes {
        let mouse_tracking = if self.mode(Mode::ANY_MOUSE) {
            MouseTracking::Motion
        } else if self.mode(Mode::BUTTON_MOUSE) {
            MouseTracking::Drag
        } else if self.mode(Mode::NORMAL_MOUSE) || self.mode(Mode::X10_MOUSE) {
            MouseTracking::Click
        } else {
            MouseTracking::Off
        };
        let mouse_encoding = if self.mode(Mode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if self.mode(Mode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Legacy
        };
        InputModes {
            app_cursor: self.mode(Mode::DECCKM),
            app_keypad: self.mode(Mode::KEYPAD_KEYS),
            bracketed_paste: self.mode(Mode::BRACKETED_PASTE),
            focus_reporting: self.mode(Mode::FOCUS_EVENT),
            line_feed_new_line: self.mode(Mode::LINEFEED),
            mouse_tracking,
            mouse_encoding,
            alternate_scroll: self.mode(Mode::ALT_SCROLL) && self.vt.alt_screen().unwrap_or(false),
        }
    }

    fn encode_key(&self, key: &KeyEvent) -> Option<Vec<u8>> {
        input::encode_key(&self.input_modes(), key)
    }

    fn encode_mouse(&self, mouse: &MouseEvent) -> Option<Vec<u8>> {
        input::encode_mouse(&self.input_modes(), mouse)
    }

    fn encode_paste(&self, text: &str, policy: &PastePolicy) -> PasteOutcome {
        input::encode_paste(&self.input_modes(), text, policy)
    }

    fn encode_focus(&self, focused: bool) -> Option<Vec<u8>> {
        input::encode_focus(&self.input_modes(), focused)
    }

    fn resize(&mut self, size: GridSize) {
        let size = clamp_size(size);
        if self
            .vt
            .resize(size.cols, size.rows, size.cell_width, size.cell_height)
            .is_ok()
        {
            // Like alacritty: a width change drops the selection, a height
            // change keeps it on its (tracked) content.
            if size.cols != self.size.cols {
                self.clear_selection();
            }
            self.size = size;
        }
        self.drain_effects();
        self.force_full = true;
    }

    fn scroll(&mut self, scroll: ViewportScroll) {
        let page = isize::try_from(self.size.rows).unwrap_or(isize::MAX);
        let delta = |n: u32| isize::try_from(n).unwrap_or(isize::MAX);
        self.vt.scroll_viewport(match scroll {
            ViewportScroll::Up(n) => ScrollViewport::Delta(-delta(n)),
            ViewportScroll::Down(n) => ScrollViewport::Delta(delta(n)),
            ViewportScroll::PageUp => ScrollViewport::Delta(-page),
            ViewportScroll::PageDown => ScrollViewport::Delta(page),
            ViewportScroll::Top => ScrollViewport::Top,
            ViewportScroll::Bottom => ScrollViewport::Bottom,
        });
    }

    fn select(&mut self, op: SelectionOp) {
        match op {
            SelectionOp::Start { at, kind } => {
                self.clear_selection();
                if let Some(anchor) = self.viewport_to_screen(at)
                    && self.set_mark(MARK_ANCHOR, anchor)
                {
                    self.selection = Some(Selection {
                        kind,
                        alt: self.vt.alt_screen().unwrap_or(false),
                        range: None,
                    });
                    self.resolve_selection(anchor, None);
                }
            }
            SelectionOp::Extend { to } => {
                if self.selection.is_none() {
                    return;
                }
                if let Some(target) = self.viewport_to_screen(to) {
                    // Like alacritty, an anchor whose content is gone is
                    // clamped to the first cell of the screen.
                    let anchor = self.mark(MARK_ANCHOR).unwrap_or(Pos::new(0, 0));
                    self.resolve_selection(anchor, Some(target));
                }
            }
            SelectionOp::Clear => self.clear_selection(),
        }
        self.force_full = true;
    }

    fn selection_text(&self) -> Option<String> {
        let kind = self.selection.as_ref()?.kind;
        let (start, end, block) = self.selection_range()?;
        let mut grid = Grid::new(&self.vt)?;
        let text = selection::text(&mut grid, kind, selection::Range { start, end, block });
        (!text.is_empty()).then_some(text)
    }

    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
        let regex = search::compile(query)?;
        let Some(mut grid) = Grid::new(&self.vt) else {
            return Ok(Vec::new());
        };
        let history = u32::try_from(self.history_rows()).unwrap_or(u32::MAX);
        Ok(search::search(
            &mut grid,
            &regex,
            query.max_matches,
            history,
        ))
    }

    fn snapshot(&mut self, frame: &mut Frame) -> Damage {
        if self
            .selection
            .as_ref()
            .is_some_and(|sel| sel.range.is_some())
            && self.mark(MARK_END).is_none()
        {
            // The selected content left the scrollback.
            self.clear_selection();
        }
        let selection = self.selection_span();
        if self.graphemes.strings.len() >= MAX_GRAPHEMES {
            self.graphemes = GraphemeTable::default();
            self.force_full = true;
        }
        let rows = usize::from(self.size.rows);
        let cols = usize::from(self.size.cols);
        let force_full = mem::take(&mut self.force_full)
            || frame.generation == 0
            || frame.rows.len() != rows
            || frame.size.cols != self.size.cols;
        frame.rows.resize_with(rows, RowView::default);
        for row in &mut frame.rows {
            row.cells.resize(cols, Cell16::default());
        }

        let mut ctx = RenderCtx {
            frame,
            damage: Damage {
                full: false,
                scrolled_rows: 0,
                rows: RowSet::with_len(rows),
            },
            graphemes: &mut self.graphemes,
            force_full,
            default_fg: 0,
            default_bg: 0,
            palette: [RgbColor::default(); 256],
            cursor: CursorState::default(),
            chars: Vec::new(),
            text: String::new(),
        };
        if self.vt.render(&mut ctx, render_state, render_row).is_err() {
            // A failed query leaves the frame partially updated; repaint it
            // all on the next snapshot rather than show stale rows.
            self.force_full = true;
        }
        let RenderCtx {
            frame,
            mut damage,
            cursor,
            ..
        } = ctx;
        if damage.full {
            damage.rows.insert_all();
        }

        let style = (cursor.shape, cursor.blinking);
        if style != self.cursor_style {
            self.cursor_style = style;
            self.events.push(TermEvent::CursorStyle {
                shape: style.0,
                blinking: style.1,
            });
        }
        let scrollbar = self.vt.scrollbar().ok();
        frame.cursor = cursor;
        frame.selection = selection;
        frame.size = self.size;
        frame.display_offset = scrollbar
            .map(|s| s.total.saturating_sub(s.offset + s.len) as usize)
            .unwrap_or(0);
        frame.history_len = self.vt.scrollback_rows().unwrap_or(0);
        frame.generation += 1;
        damage
    }

    fn take_events(&mut self, out: &mut Vec<TermEvent>) {
        out.append(&mut self.events);
    }

    fn grapheme(&self, glyph: u32) -> Option<&str> {
        self.graphemes.get(glyph)
    }

    fn answer_clipboard_load(&mut self, id: u64, text: Option<&str>) {
        let Some(pos) = self
            .pending_loads
            .iter()
            .position(|(pending, _)| *pending == id)
        else {
            return;
        };
        let Some((_, load)) = self.pending_loads.remove(pos) else {
            return;
        };
        if let Some(text) = text {
            self.replies.extend_from_slice(&load.reply(text));
        }
    }

    /// Ghostty does not expose its page allocation, so this assumes every
    /// history and screen row costs `cols` packed 8-byte cells plus an 8-byte
    /// header, plus the render state's copy of the viewport. Pages are
    /// allocated whole, so the real figure is higher; measure RSS for truth.
    ///
    /// Idle compression ([`TermCore::on_idle`]) is not reflected: Ghostty
    /// reports neither which pages are compressed nor their size, so after
    /// compression this is an upper bound.
    fn memory_estimate(&self) -> usize {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        let lines = self.vt.total_rows().unwrap_or(rows) + rows;
        let row_bytes = cols * GHOSTTY_CELL_BYTES + GHOSTTY_ROW_BYTES;
        // Render state keeps a raw cell plus a style per visible cell.
        let render_bytes = rows * cols * (GHOSTTY_CELL_BYTES + mem::size_of::<style::Style>());
        lines * row_bytes + render_bytes + self.graphemes.bytes()
    }

    /// Compresses the scrollback ([`GhosttyTerm::compress_scrollback`]).
    /// Compressed pages are restored transparently when read (scrolling back,
    /// selection, search), so visible content is unaffected; the next idle
    /// pass compresses them again. Every call does a full pass: reads restore
    /// pages without changing Ghostty's compression activity token, so the
    /// token cannot tell when a pass is redundant.
    fn on_idle(&mut self) {
        self.compress_scrollback();
    }
}

/// Reads the global render state and decides whether every row is repainted.
fn render_state(
    ctx: &mut RenderCtx<'_>,
    snap: &termax_term_ghostty_sys::vt::render::Snapshot<'static, '_>,
) -> VtResult<bool> {
    let colors = snap.colors()?;
    ctx.default_fg = rgba(colors.foreground);
    ctx.default_bg = rgba(colors.background);
    ctx.palette = colors.palette;
    let shape = match snap.cursor_visual_style()? {
        CursorVisualStyle::Bar => CursorShape::Beam,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::BlockHollow => CursorShape::HollowBlock,
        _ => CursorShape::Block,
    };
    let (pos, in_view) = match snap.cursor_viewport()? {
        Some(c) => {
            let col = if c.at_wide_tail {
                c.x.saturating_sub(1)
            } else {
                c.x
            };
            (ViewportPoint::new(c.y, col), true)
        }
        None => (ViewportPoint::default(), false),
    };
    ctx.cursor = CursorState {
        pos,
        shape,
        visible: in_view && snap.cursor_visible()?,
        blinking: snap.cursor_blinking()?,
    };
    let full = ctx.force_full || snap.dirty()? == Dirty::Full;
    ctx.damage.full = full;
    Ok(full)
}

/// Converts one viewport row into the frame.
fn render_row(
    ctx: &mut RenderCtx<'_>,
    index: usize,
    row: &RowIteration<'static, '_>,
    cells: &mut CellIteration<'static, '_>,
) -> VtResult<()> {
    let Some(out_row) = ctx.frame.rows.get_mut(index) else {
        return Ok(());
    };
    ctx.damage.rows.insert(index);
    let raw_row = row.raw_row()?;
    let row_has_graphemes = raw_row.has_grapheme_cluster()?;
    let row_has_links = raw_row.has_hyperlink()?;
    let mut col = 0;
    while let Some(cell) = cells.next() {
        let Some(out) = out_row.cells.get_mut(col) else {
            break;
        };
        col += 1;
        let raw = cell.raw_cell()?;
        let tag = raw.content_tag()?;
        let mut flags = match raw.wide()? {
            CellWide::Wide => cell_flags::WIDE,
            CellWide::SpacerTail => cell_flags::WIDE_SPACER,
            CellWide::Narrow | CellWide::SpacerHead => 0,
        };
        if row_has_links && raw.has_hyperlink()? {
            flags |= cell_flags::HYPERLINK;
        }

        let glyph = match tag {
            CellContentTag::CodepointGrapheme if row_has_graphemes => {
                let len = cell.graphemes_len()?;
                ctx.chars.clear();
                ctx.chars.resize(len, '\0');
                cell.graphemes_buf(&mut ctx.chars)?;
                ctx.text.clear();
                ctx.text.extend(ctx.chars.iter());
                ctx.graphemes.intern(&ctx.text)
            }
            CellContentTag::Codepoint | CellContentTag::CodepointGrapheme => {
                match raw.codepoint()? {
                    0 => u32::from(' '),
                    cp => cp,
                }
            }
            CellContentTag::BgColorPalette | CellContentTag::BgColorRgb => u32::from(' '),
        };

        let (fg, bg, ul_color) = if raw.has_styling()? {
            let s = cell.style()?;
            flags |= style_flags(&s);
            let ul_color = match s.underline_color {
                StyleColor::None => 0,
                StyleColor::Rgb(c) => Cell16::pack_ul_color(rgba(c)),
                StyleColor::Palette(i) => {
                    Cell16::pack_ul_color(rgba(ctx.palette[usize::from(i.0)]))
                }
            };
            (
                cell.fg_color()?.map_or(ctx.default_fg, rgba),
                cell.bg_color()?.map_or(ctx.default_bg, rgba),
                ul_color,
            )
        } else if matches!(
            tag,
            CellContentTag::BgColorPalette | CellContentTag::BgColorRgb
        ) {
            (
                ctx.default_fg,
                cell.bg_color()?.map_or(ctx.default_bg, rgba),
                0,
            )
        } else {
            (ctx.default_fg, ctx.default_bg, 0)
        };
        *out = Cell16 {
            glyph,
            fg,
            bg,
            flags,
            ul_color,
        };
    }
    // Ghostty rows always span the grid width; blank any remainder defensively.
    for out in out_row.cells.iter_mut().skip(col) {
        *out = Cell16 {
            glyph: u32::from(' '),
            fg: ctx.default_fg,
            bg: ctx.default_bg,
            flags: 0,
            ul_color: 0,
        };
    }
    Ok(())
}

fn style_flags(s: &style::Style) -> u16 {
    let mut flags = 0;
    for (on, bit) in [
        (s.bold, cell_flags::BOLD),
        (s.italic, cell_flags::ITALIC),
        (s.faint, cell_flags::DIM),
        (s.inverse, cell_flags::INVERSE),
        (s.invisible, cell_flags::HIDDEN),
        (s.strikethrough, cell_flags::STRIKE),
        (s.blink, cell_flags::BLINK),
    ] {
        if on {
            flags |= bit;
        }
    }
    let underline: u16 = match s.underline {
        Underline::Single => 1,
        Underline::Double => 2,
        Underline::Curly => 3,
        Underline::Dotted => 4,
        Underline::Dashed => 5,
        _ => 0,
    };
    flags | (underline << cell_flags::UNDERLINE_SHIFT)
}

fn rgba(c: RgbColor) -> u32 {
    (u32::from(c.r) << 24) | (u32::from(c.g) << 16) | (u32::from(c.b) << 8) | 0xff
}

fn clamp_size(size: GridSize) -> GridSize {
    GridSize {
        cols: size.cols.max(2),
        rows: size.rows.max(1),
        ..size
    }
}

/// Converts a scrollback line count into Ghostty's byte budget.
///
/// Ghostty stores rows in fixed-size pages (~400 KiB whatever the width) and
/// drops the oldest whole page once one more page would exceed the budget.
/// The budget therefore covers the rows at 8 bytes per cell plus header with
/// ~6% page overhead, plus two pages of slack so at least `lines` survive.
/// The budget is fixed at construction: after a resize the line count it
/// buys changes with the width. `0` keeps no history.
fn scrollback_bytes(lines: usize, cols: u16) -> usize {
    const PAGE_SLACK: usize = 2 * 512 * 1024;
    if lines == 0 {
        return 0;
    }
    let row = usize::from(cols) * GHOSTTY_CELL_BYTES + GHOSTTY_ROW_BYTES;
    lines
        .saturating_mul(row / 16 * 17)
        .saturating_add(PAGE_SLACK)
}

/// DA1 `CSI ? 6 c` (VT102), matching the alacritty backend's answer.
fn device_attributes() -> DeviceAttributes {
    DeviceAttributes {
        primary: PrimaryDeviceAttributes::new(ConformanceLevel(6), &[]),
        secondary: SecondaryDeviceAttributes {
            device_type: DeviceType(0),
            firmware_version: 0,
            rom_cartridge: 0,
        },
        tertiary: TertiaryDeviceAttributes { unit_id: 0 },
    }
}

/// Default colours: the configured ANSI 16 on top of Ghostty's xterm cube and
/// grey ramp (the same values `termax_term` uses for 16..=255).
fn ghostty_colors(p: &Palette) -> (RgbColor, RgbColor, style::Palette) {
    let conv = |c: termax_term::Rgb| RgbColor {
        r: c.r,
        g: c.g,
        b: c.b,
    };
    let mut palette = style::Palette::default();
    for (slot, c) in palette.0.iter_mut().zip(p.ansi) {
        *slot = conv(c);
    }
    (conv(p.foreground), conv(p.background), palette)
}
