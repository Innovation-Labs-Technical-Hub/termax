//! Experimental [`TermCore`] backend on Ghostty's terminal core (libghostty-vt).
//!
//! Built to compare parser throughput and per-tab memory with the alacritty
//! backend behind the same trait. Building needs Zig 0.15.2 on `PATH`; the
//! first build clones Ghostty at the commit pinned by `libghostty-vt-sys`.
//!
//! Not supported yet (the methods are inert and documented as such): text
//! selection, search, OSC 52 clipboard reads, OSC 133 prompt marks, colour
//! query replies beyond what Ghostty answers itself, and reset-title events.
//! `CursorStyle` events are raised at [`TermCore::snapshot`] time rather than
//! while feeding, because Ghostty only exposes the cursor shape through its
//! render state.

use std::collections::HashMap;
use std::mem;
use std::time::Instant;

use termax_term::input::{self, InputModes, KeyEvent, MouseEvent, PasteOutcome, PastePolicy};
use termax_term::{
    Budget, Cell16, ClipboardStoreMode, ClipboardTarget, CursorShape, CursorState, Damage, Fed,
    Frame, GRAPHEME_BIT, GridSize, Match, MouseEncoding, MouseTracking, Palette, RowSet, RowView,
    SearchError, SearchQuery, SelectionOp, TermConfig, TermCore, TermEvent, ViewportPoint,
    ViewportScroll, cell_flags,
};
use termax_term_ghostty_sys::vt::error::{Error as VtError, Result as VtResult};
use termax_term_ghostty_sys::vt::render::{CellIteration, CursorVisualStyle, Dirty, RowIteration};
use termax_term_ghostty_sys::vt::screen::{CellContentTag, CellWide};
use termax_term_ghostty_sys::vt::style::{self, RgbColor, StyleColor, Underline};
use termax_term_ghostty_sys::vt::terminal::{
    ClipboardLocation, CompressionMode, CompressionResult, ConformanceLevel, DeviceAttributes,
    DeviceType, Mode, PrimaryDeviceAttributes, ScrollViewport, SecondaryDeviceAttributes,
    TertiaryDeviceAttributes,
};
use termax_term_ghostty_sys::{Effect, Vt, VtOptions};

/// Input is handed to the parser in chunks of this size; the time budget is
/// checked between chunks.
const FEED_CHUNK: usize = 16 * 1024;

/// Grapheme table size at which it is reset (forcing a full repaint).
const MAX_GRAPHEMES: usize = 65_536;

/// Bytes Ghostty stores per cell (`page.Cell` is a packed `u64`).
const GHOSTTY_CELL_BYTES: usize = 8;

/// Bytes Ghostty stores per row header (`page.Row` is a packed `u64`).
const GHOSTTY_ROW_BYTES: usize = 8;

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

/// A terminal emulator instance backed by Ghostty's libghostty-vt.
pub struct GhosttyTerm {
    vt: Vt,
    replies: Vec<u8>,
    events: Vec<TermEvent>,
    config: TermConfig,
    graphemes: GraphemeTable,
    force_full: bool,
    size: GridSize,
    cursor_style: (CursorShape, bool),
}

impl GhosttyTerm {
    /// Creates a terminal with the given configuration.
    ///
    /// # Panics
    ///
    /// Only if Ghostty cannot allocate the terminal; use [`Self::try_new`] to
    /// handle that.
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
            replies: Vec::new(),
            events: Vec::new(),
            config: TermConfig { size, ..config },
            graphemes: GraphemeTable::default(),
            force_full: true,
            size,
            cursor_style: (CursorShape::Block, false),
        })
    }

    /// Runs Ghostty's scrollback compression to completion (not part of
    /// [`TermCore`]; an embedder would call it incrementally when idle).
    /// Returns `false` where the platform cannot reclaim the memory.
    pub fn compress_scrollback(&mut self) -> bool {
        matches!(
            self.vt.compress(CompressionMode::Full),
            Ok(CompressionResult::Complete | CompressionResult::Pending)
        )
    }

    fn drain_effects(&mut self) {
        let mut fx = self.vt.effects();
        self.replies.append(&mut fx.pty);
        for effect in fx.events.drain(..) {
            match effect {
                Effect::Bell => self.events.push(TermEvent::Bell),
                Effect::Title(title) => self.events.push(TermEvent::Title(title)),
                Effect::Pwd(pwd) => {
                    let (host, path) = parse_file_url(&pwd);
                    self.events.push(TermEvent::Cwd { host, path });
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
}

impl TermCore for GhosttyTerm {
    fn feed(&mut self, bytes: &[u8], budget: Budget) -> Fed {
        let limit = bytes.len().min(budget.max_bytes);
        let started = Instant::now();
        let mut consumed = 0;
        while consumed < limit {
            let end = (consumed + FEED_CHUNK).min(limit);
            self.vt.write(&bytes[consumed..end]);
            self.drain_effects();
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

    /// Not supported yet: libghostty-vt 0.2 only offers pixel-driven selection
    /// gestures. Ignored.
    fn select(&mut self, _op: SelectionOp) {}

    /// Not supported yet; always `None`.
    fn selection_text(&self) -> Option<String> {
        None
    }

    /// Not supported yet (libghostty-vt 0.2 has no search API); always empty.
    fn search(&mut self, _query: &SearchQuery) -> Result<Vec<Match>, SearchError> {
        Ok(Vec::new())
    }

    fn snapshot(&mut self, frame: &mut Frame) -> Damage {
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
        frame.selection = None;
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

    /// Clipboard reads never reach the embedder (Ghostty's lib-vt drops OSC 52
    /// `?`), so there is nothing to answer.
    fn answer_clipboard_load(&mut self, _id: u64, _text: Option<&str>) {}

    /// Ghostty does not expose its page allocation, so this assumes every
    /// history and screen row costs `cols` packed 8-byte cells plus an 8-byte
    /// header, plus the render state's copy of the viewport. Pages are
    /// allocated whole, so the real figure is higher; measure RSS for truth.
    fn memory_estimate(&self) -> usize {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        let lines = self.vt.total_rows().unwrap_or(rows) + rows;
        let row_bytes = cols * GHOSTTY_CELL_BYTES + GHOSTTY_ROW_BYTES;
        // Render state keeps a raw cell plus a style per visible cell.
        let render_bytes = rows * cols * (GHOSTTY_CELL_BYTES + mem::size_of::<style::Style>());
        lines * row_bytes + render_bytes + self.graphemes.bytes()
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

/// Splits an OSC 7 `file://host/path` URL into host and percent-decoded path.
/// Anything else is reported verbatim as a path.
fn parse_file_url(url: &str) -> (Option<String>, String) {
    let Some(rest) = url.strip_prefix("file://") else {
        return (None, url.to_owned());
    };
    let (host, path) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    let host = (!host.is_empty()).then(|| host.to_owned());
    (host, percent_decode(path))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_split_and_decode() {
        assert_eq!(
            parse_file_url("file://host/tmp/a%20b"),
            (Some("host".into()), "/tmp/a b".into())
        );
        assert_eq!(parse_file_url("file:///x"), (None, "/x".into()));
        assert_eq!(parse_file_url("/plain"), (None, "/plain".into()));
        assert_eq!(percent_decode("%zz%4"), "%zz%4");
    }
}
