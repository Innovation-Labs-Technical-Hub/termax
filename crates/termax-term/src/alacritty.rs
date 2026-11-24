//! [`TermCore`] backend built on `alacritty_terminal`.

use std::collections::{HashMap, VecDeque};
use std::mem;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::{RegexIter, RegexSearch};
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{self, Color, CursorShape as AnsiCursorShape};

use crate::TermCore;
use crate::input::{
    self, InputModes, KeyEvent, MouseEncoding, MouseEvent, MouseTracking, PasteOutcome, PastePolicy,
};
use crate::osc_scan::{OscEvent, OscScanner, ScanAction};
use crate::palette::{Palette, ResolvedPalette, Rgb};
use crate::types::{
    Budget, Cell16, ClipboardLoadMode, ClipboardPolicy, ClipboardStoreMode, ClipboardTarget,
    CursorShape, CursorState, Damage, Fed, Frame, GRAPHEME_BIT, GridPoint, GridSize, Match, RowSet,
    RowView, SearchError, SearchQuery, SelectionKind, SelectionOp, SelectionSpan, TermEvent,
    ViewportPoint, ViewportScroll, cell_flags,
};

/// Input is handed to the parser in chunks of this size; the time budget is
/// checked between chunks.
const FEED_CHUNK: usize = 4 * 1024;

/// Grapheme table size at which it is reset (forcing a full repaint).
const MAX_GRAPHEMES: usize = 65_536;

/// Unanswered clipboard-load requests kept before the oldest is dropped.
const MAX_PENDING_LOADS: usize = 16;

/// Construction options for [`AlacrittyTerm`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermConfig {
    /// Initial size.
    pub size: GridSize,
    /// Maximum scrollback lines kept for the primary screen.
    pub scrollback_lines: usize,
    /// Default colours.
    pub palette: Palette,
    /// OSC 52 policy.
    pub clipboard: ClipboardPolicy,
}

impl Default for TermConfig {
    fn default() -> Self {
        Self {
            size: GridSize::default(),
            scrollback_lines: 10_000,
            palette: Palette::default(),
            clipboard: ClipboardPolicy::default(),
        }
    }
}

/// Collects alacritty events; shared with the `Term` that owns a clone.
#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// Size adapter for alacritty's `Dimensions`.
struct Dims {
    cols: usize,
    rows: usize,
}

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// Interns multi-codepoint graphemes so cells stay 16 bytes.
#[derive(Default)]
struct GraphemeTable {
    strings: Vec<String>,
    index: HashMap<String, u32>,
    scratch: String,
}

impl GraphemeTable {
    fn intern(&mut self, base: char, zerowidth: &[char]) -> u32 {
        self.scratch.clear();
        self.scratch.push(base);
        self.scratch.extend(zerowidth);
        if let Some(&id) = self.index.get(self.scratch.as_str()) {
            return GRAPHEME_BIT | id;
        }
        // Bounded by MAX_GRAPHEMES plus one screen, far below 2^31.
        let id = self.strings.len() as u32;
        self.strings.push(self.scratch.clone());
        self.index.insert(self.scratch.clone(), id);
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

type LoadFormatter = Arc<dyn Fn(&str) -> String + Sync + Send + 'static>;

/// A terminal emulator instance backed by `alacritty_terminal`.
pub struct AlacrittyTerm {
    term: Term<Listener>,
    parser: ansi::Processor,
    queue: Listener,
    spare_events: Vec<Event>,
    scanner: OscScanner,
    actions: Vec<ScanAction>,
    replies: Vec<u8>,
    events: Vec<TermEvent>,
    palette: ResolvedPalette,
    clipboard: ClipboardPolicy,
    graphemes: GraphemeTable,
    selection_anchor: Option<Point>,
    force_full: bool,
    size: GridSize,
    next_load_id: u64,
    pending_loads: VecDeque<(u64, LoadFormatter)>,
    cursor_style: ansi::CursorStyle,
}

impl AlacrittyTerm {
    /// Creates a terminal with the given configuration.
    pub fn new(config: TermConfig) -> Self {
        let size = config.size.clamped();
        let queue = Listener::default();
        let term_config = Config {
            scrolling_history: config.scrollback_lines,
            // Both directions reach our handler, which applies `ClipboardPolicy`.
            osc52: Osc52::CopyPaste,
            kitty_keyboard: false,
            ..Config::default()
        };
        let dims = Dims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        };
        let term = Term::new(term_config, &dims, queue.clone());
        let cursor_style = term.cursor_style();
        Self {
            term,
            parser: ansi::Processor::new(),
            queue,
            spare_events: Vec::new(),
            scanner: OscScanner::default(),
            actions: Vec::new(),
            replies: Vec::new(),
            events: Vec::new(),
            palette: ResolvedPalette::new(&config.palette),
            clipboard: config.clipboard,
            graphemes: GraphemeTable::default(),
            selection_anchor: None,
            force_full: true,
            size,
            next_load_id: 1,
            pending_loads: VecDeque::new(),
            cursor_style,
        }
    }

    /// Replaces the OSC 52 policy.
    pub fn set_clipboard_policy(&mut self, policy: ClipboardPolicy) {
        self.clipboard = policy;
        if policy.load == ClipboardLoadMode::Deny {
            self.pending_loads.clear();
        }
    }

    /// Replaces the default colours and repaints everything.
    pub fn set_palette(&mut self, palette: &Palette) {
        self.palette = ResolvedPalette::new(palette);
        self.force_full = true;
    }

    /// Ends a synchronized update (mode 2026) whose deadline passed, so a host
    /// that never sends the end marker cannot freeze the screen.
    fn expire_sync(&mut self) {
        if let Some(deadline) = self.parser.sync_timeout().sync_timeout()
            && Instant::now() >= deadline
        {
            self.parser.stop_sync(&mut self.term);
            self.pump_events();
        }
    }

    fn process_chunk(&mut self, chunk: &[u8]) {
        let mut actions = mem::take(&mut self.actions);
        actions.clear();
        self.scanner.scan(chunk, &mut actions);
        for action in actions.drain(..) {
            match action {
                ScanAction::Forward(range) => self.parser.advance(&mut self.term, &chunk[range]),
                ScanAction::Inject(bytes) => self.parser.advance(&mut self.term, bytes),
                ScanAction::Event(event) => {
                    // Keep emulator events that preceded the OSC in order.
                    self.pump_events();
                    self.push_osc_event(event);
                }
            }
        }
        self.actions = actions;
        self.pump_events();
    }

    fn push_osc_event(&mut self, event: OscEvent) {
        let event = match event {
            OscEvent::Cwd { host, path } => TermEvent::Cwd { host, path },
            OscEvent::Prompt(kind) => TermEvent::PromptMark {
                kind,
                cursor_line: u16::try_from(self.term.grid().cursor.point.line.0.max(0))
                    .unwrap_or(u16::MAX),
            },
        };
        self.events.push(event);
    }

    /// Translates queued alacritty events into replies and [`TermEvent`]s.
    fn pump_events(&mut self) {
        let mut batch = mem::take(&mut self.spare_events);
        {
            let mut queue = self.queue.0.lock().unwrap_or_else(PoisonError::into_inner);
            mem::swap(&mut *queue, &mut batch);
        }
        for event in batch.drain(..) {
            match event {
                Event::PtyWrite(text) => self.replies.extend_from_slice(text.as_bytes()),
                Event::Title(title) => self.events.push(TermEvent::Title(title)),
                Event::ResetTitle => self.events.push(TermEvent::ResetTitle),
                Event::Bell => self.events.push(TermEvent::Bell),
                Event::ClipboardStore(ty, text) => self.clipboard_store(ty, text),
                Event::ClipboardLoad(ty, formatter) => self.clipboard_load(ty, formatter),
                Event::ColorRequest(index, formatter) => {
                    let rgb = resolve_rgb(self.term.colors(), &self.palette, index);
                    let reply = formatter(ansi::Rgb {
                        r: rgb.r,
                        g: rgb.g,
                        b: rgb.b,
                    });
                    self.replies.extend_from_slice(reply.as_bytes());
                }
                Event::TextAreaSizeRequest(formatter) => {
                    if self.size.cell_width > 0 && self.size.cell_height > 0 {
                        let reply = formatter(WindowSize {
                            num_lines: self.size.rows,
                            num_cols: self.size.cols,
                            cell_width: self.size.cell_width,
                            cell_height: self.size.cell_height,
                        });
                        self.replies.extend_from_slice(reply.as_bytes());
                    }
                }
                Event::CursorBlinkingChange => {
                    let style = self.term.cursor_style();
                    if style != self.cursor_style {
                        self.cursor_style = style;
                        self.events.push(TermEvent::CursorStyle {
                            shape: map_shape(style.shape).unwrap_or_default(),
                            blinking: style.blinking,
                        });
                    }
                }
                Event::MouseCursorDirty | Event::Wakeup | Event::Exit | Event::ChildExit(_) => {}
            }
        }
        self.spare_events = batch;
    }

    fn clipboard_store(&mut self, ty: ClipboardType, text: String) {
        if text.len() > self.clipboard.max_store_bytes {
            return;
        }
        let target = map_clipboard(ty);
        match self.clipboard.store {
            ClipboardStoreMode::Deny => {}
            ClipboardStoreMode::Allow => {
                self.events.push(TermEvent::ClipboardStore { target, text })
            }
            ClipboardStoreMode::Ask => {
                self.events
                    .push(TermEvent::ClipboardStoreRequest { target, text });
            }
        }
    }

    fn clipboard_load(&mut self, ty: ClipboardType, formatter: LoadFormatter) {
        if self.clipboard.load == ClipboardLoadMode::Deny {
            return;
        }
        let id = self.next_load_id;
        self.next_load_id += 1;
        if self.pending_loads.len() >= MAX_PENDING_LOADS {
            self.pending_loads.pop_front();
        }
        self.pending_loads.push_back((id, formatter));
        self.events.push(TermEvent::ClipboardLoadRequest {
            id,
            target: map_clipboard(ty),
        });
    }

    /// Viewport point to buffer point, clamped to the grid.
    fn viewport_to_point(&self, p: ViewportPoint) -> Point {
        let row = usize::from(p.row).min(self.term.screen_lines() - 1);
        let col = usize::from(p.col).min(self.term.columns() - 1);
        let offset = self.term.grid().display_offset();
        Point::new(Line(row as i32 - offset as i32), Column(col))
    }

    fn cursor_state(&self) -> CursorState {
        let grid = self.term.grid();
        let mut point = grid.cursor.point;
        if grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) && point.column.0 > 0 {
            point.column -= 1;
        }
        let style = self.term.cursor_style();
        let shape = map_shape(style.shape);
        let row = point.line.0 + grid.display_offset() as i32;
        let in_view = (0..self.term.screen_lines() as i32).contains(&row);
        let shown = self.term.mode().contains(TermMode::SHOW_CURSOR) && shape.is_some();
        CursorState {
            pos: ViewportPoint::new(
                row.clamp(0, i32::from(u16::MAX)) as u16,
                point.column.0 as u16,
            ),
            shape: shape.unwrap_or_default(),
            visible: shown && in_view,
            blinking: style.blinking,
        }
    }

    fn selection_span(&self) -> Option<SelectionSpan> {
        let range = self.term.selection.as_ref()?.to_range(&self.term)?;
        let offset = self.term.grid().display_offset() as i32;
        let rows = self.term.screen_lines() as i32;
        let last_col = self.term.columns() as u16 - 1;
        let start_row = range.start.line.0 + offset;
        let end_row = range.end.line.0 + offset;
        if end_row < 0 || start_row >= rows {
            return None;
        }
        let block = range.is_block;
        let start = if start_row < 0 {
            ViewportPoint::new(
                0,
                if block {
                    range.start.column.0 as u16
                } else {
                    0
                },
            )
        } else {
            ViewportPoint::new(start_row as u16, range.start.column.0 as u16)
        };
        let end = if end_row >= rows {
            ViewportPoint::new(
                (rows - 1) as u16,
                if block {
                    range.end.column.0 as u16
                } else {
                    last_col
                },
            )
        } else {
            ViewportPoint::new(end_row as u16, range.end.column.0 as u16)
        };
        Some(SelectionSpan { start, end, block })
    }
}

impl TermCore for AlacrittyTerm {
    fn feed(&mut self, bytes: &[u8], budget: Budget) -> Fed {
        self.expire_sync();
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
        let mode = *self.term.mode();
        let mouse_tracking = if mode.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Click
        } else {
            MouseTracking::Off
        };
        let mouse_encoding = if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Legacy
        };
        InputModes {
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            app_keypad: mode.contains(TermMode::APP_KEYPAD),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
            focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
            line_feed_new_line: mode.contains(TermMode::LINE_FEED_NEW_LINE),
            mouse_tracking,
            mouse_encoding,
            alternate_scroll: mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL),
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
        let size = size.clamped();
        self.size = size;
        self.term.resize(Dims {
            cols: usize::from(size.cols),
            rows: usize::from(size.rows),
        });
        if self.term.selection.is_none() {
            self.selection_anchor = None;
        }
        self.force_full = true;
    }

    fn scroll(&mut self, scroll: ViewportScroll) {
        let scroll = match scroll {
            ViewportScroll::Up(n) => Scroll::Delta(i32::try_from(n).unwrap_or(i32::MAX)),
            ViewportScroll::Down(n) => Scroll::Delta(-i32::try_from(n).unwrap_or(i32::MAX)),
            ViewportScroll::PageUp => Scroll::PageUp,
            ViewportScroll::PageDown => Scroll::PageDown,
            ViewportScroll::Top => Scroll::Top,
            ViewportScroll::Bottom => Scroll::Bottom,
        };
        self.term.scroll_display(scroll);
        self.pump_events();
    }

    fn select(&mut self, op: SelectionOp) {
        match op {
            SelectionOp::Start { at, kind } => {
                let point = self.viewport_to_point(at);
                let ty = match kind {
                    SelectionKind::Char => SelectionType::Simple,
                    SelectionKind::Word => SelectionType::Semantic,
                    SelectionKind::Line => SelectionType::Lines,
                    SelectionKind::Block => SelectionType::Block,
                };
                self.term.selection = Some(Selection::new(ty, point, Side::Left));
                self.selection_anchor = Some(point);
            }
            SelectionOp::Extend { to } => {
                let point = self.viewport_to_point(to);
                let anchor = self.selection_anchor;
                if let Some(selection) = self.term.selection.as_mut() {
                    let side = match anchor {
                        Some(a) if point < a => Side::Left,
                        _ => Side::Right,
                    };
                    selection.update(point, side);
                }
            }
            SelectionOp::Clear => {
                self.term.selection = None;
                self.selection_anchor = None;
            }
        }
        self.force_full = true;
    }

    fn selection_text(&self) -> Option<String> {
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
        // alacritty applies smart-case; an explicit inline flag overrides it.
        let flag = if query.case_insensitive {
            "(?i)"
        } else {
            "(?-i)"
        };
        let pattern = format!("{flag}{}", query.pattern);
        let mut regex =
            RegexSearch::new(&pattern).map_err(|e| SearchError::InvalidPattern(e.to_string()))?;
        let term = &self.term;
        let start = Point::new(term.topmost_line(), Column(0));
        let end = Point::new(term.bottommost_line(), term.last_column());
        let to_grid = |p: Point| GridPoint {
            line: p.line.0,
            col: p.column.0 as u16,
        };
        Ok(
            RegexIter::new(start, end, Direction::Right, term, &mut regex)
                .take(query.max_matches)
                .map(|m| Match {
                    start: to_grid(*m.start()),
                    end: to_grid(*m.end()),
                })
                .collect(),
        )
    }

    fn snapshot(&mut self, frame: &mut Frame) -> Damage {
        self.expire_sync();
        if self.graphemes.strings.len() >= MAX_GRAPHEMES {
            self.graphemes = GraphemeTable::default();
            self.force_full = true;
        }

        let rows = self.term.screen_lines();
        let cols = self.term.columns();
        let mut full = mem::take(&mut self.force_full)
            || frame.generation == 0
            || frame.rows.len() != rows
            || usize::from(frame.size.cols) != cols;
        frame.rows.resize_with(rows, RowView::default);
        for row in &mut frame.rows {
            row.cells.resize(cols, Cell16::default());
        }

        let mut damage = Damage {
            full: false,
            scrolled_rows: 0,
            rows: RowSet::with_len(rows),
        };
        match self.term.damage() {
            TermDamage::Full => full = true,
            TermDamage::Partial(lines) => {
                for bounds in lines {
                    damage.rows.insert(bounds.line);
                }
            }
        }
        self.term.reset_damage();
        if full {
            damage.full = true;
            damage.rows.insert_all();
        }

        let term = &self.term;
        let graphemes = &mut self.graphemes;
        let palette = &self.palette;
        let colors = term.colors();
        let offset = term.grid().display_offset() as i32;
        for row in damage.rows.iter() {
            let grid_row = &term.grid()[Line(row as i32 - offset)];
            for (col, out) in frame.rows[row].cells.iter_mut().enumerate() {
                *out = convert_cell(&grid_row[Column(col)], colors, palette, graphemes);
            }
        }

        frame.cursor = self.cursor_state();
        frame.selection = self.selection_span();
        frame.size = self.size;
        frame.display_offset = self.term.grid().display_offset();
        frame.history_len = self.term.history_size();
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
        let Some((_, formatter)) = self.pending_loads.remove(pos) else {
            return;
        };
        if let Some(text) = text {
            self.replies.extend_from_slice(formatter(text).as_bytes());
        }
    }

    fn memory_estimate(&self) -> usize {
        let grid = self.term.grid();
        let cols = grid.columns();
        let row_bytes = cols * mem::size_of::<Cell>() + 3 * mem::size_of::<usize>();
        // Active grid incl. history, plus the inactive (alternate or primary) screen.
        let lines = grid.total_lines() + grid.screen_lines();
        lines * row_bytes + self.graphemes.bytes() + self.parser.sync_bytes_count()
    }
}

fn map_clipboard(ty: ClipboardType) -> ClipboardTarget {
    match ty {
        ClipboardType::Clipboard => ClipboardTarget::Clipboard,
        ClipboardType::Selection => ClipboardTarget::Selection,
    }
}

/// `None` for a hidden cursor.
fn map_shape(shape: AnsiCursorShape) -> Option<CursorShape> {
    match shape {
        AnsiCursorShape::Block => Some(CursorShape::Block),
        AnsiCursorShape::Underline => Some(CursorShape::Underline),
        AnsiCursorShape::Beam => Some(CursorShape::Beam),
        AnsiCursorShape::HollowBlock => Some(CursorShape::HollowBlock),
        AnsiCursorShape::Hidden => None,
    }
}

/// Palette entry `index`, honouring runtime overrides from OSC 4/10/11/12.
fn resolve_rgb(overrides: &Colors, palette: &ResolvedPalette, index: usize) -> Rgb {
    match overrides[index.min(crate::palette::COLOR_COUNT - 1)] {
        Some(c) => Rgb::new(c.r, c.g, c.b),
        None => palette.get(index),
    }
}

fn resolve_color(color: Color, overrides: &Colors, palette: &ResolvedPalette) -> u32 {
    match color {
        Color::Spec(c) => Rgb::new(c.r, c.g, c.b).rgba(),
        Color::Indexed(i) => resolve_rgb(overrides, palette, usize::from(i)).rgba(),
        Color::Named(n) => resolve_rgb(overrides, palette, n as usize).rgba(),
    }
}

fn convert_cell(
    cell: &Cell,
    colors: &Colors,
    palette: &ResolvedPalette,
    graphemes: &mut GraphemeTable,
) -> Cell16 {
    let glyph = match cell.zerowidth() {
        Some(zw) if !zw.is_empty() => graphemes.intern(cell.c, zw),
        _ => cell.c as u32,
    };

    let f = cell.flags;
    let mut flags = 0u16;
    let mut set = |cond: bool, bit: u16| {
        if cond {
            flags |= bit;
        }
    };
    set(f.contains(Flags::BOLD), cell_flags::BOLD);
    set(f.contains(Flags::ITALIC), cell_flags::ITALIC);
    set(f.contains(Flags::DIM), cell_flags::DIM);
    set(f.contains(Flags::INVERSE), cell_flags::INVERSE);
    set(f.contains(Flags::HIDDEN), cell_flags::HIDDEN);
    set(f.contains(Flags::STRIKEOUT), cell_flags::STRIKE);
    set(f.contains(Flags::WIDE_CHAR), cell_flags::WIDE);
    set(f.contains(Flags::WIDE_CHAR_SPACER), cell_flags::WIDE_SPACER);
    set(cell.hyperlink().is_some(), cell_flags::HYPERLINK);
    let underline: u16 = if f.contains(Flags::DOUBLE_UNDERLINE) {
        2
    } else if f.contains(Flags::UNDERCURL) {
        3
    } else if f.contains(Flags::DOTTED_UNDERLINE) {
        4
    } else if f.contains(Flags::DASHED_UNDERLINE) {
        5
    } else {
        u16::from(f.contains(Flags::UNDERLINE))
    };
    flags |= underline << cell_flags::UNDERLINE_SHIFT;

    let ul_color = cell
        .underline_color()
        .map(|c| Cell16::pack_ul_color(resolve_color(c, colors, palette)))
        .unwrap_or(0);

    Cell16 {
        glyph,
        fg: resolve_color(cell.fg, colors, palette),
        bg: resolve_color(cell.bg, colors, palette),
        flags,
        ul_color,
    }
}
