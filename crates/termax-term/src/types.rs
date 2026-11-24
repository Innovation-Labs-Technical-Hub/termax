//! Backend-neutral data types exchanged across the [`crate::TermCore`] boundary.

use std::time::Duration;

/// Upper bounds for one [`crate::TermCore::feed`] call.
///
/// The byte budget is a hard limit. The time budget is checked between internal
/// chunks, so a call may overrun it by the cost of one chunk; at least one chunk
/// is always processed so the caller makes progress even with a zero time budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Maximum number of input bytes to consume.
    pub max_bytes: usize,
    /// Soft wall-clock limit for the call.
    pub max_time: Duration,
}

impl Budget {
    /// A budget with no practical limit, useful for tests and replay.
    pub const UNLIMITED: Budget = Budget {
        max_bytes: usize::MAX,
        max_time: Duration::MAX,
    };
}

impl Default for Budget {
    /// 64 KiB / 2 ms, sized so a parser task never starves the UI frame.
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024,
            max_time: Duration::from_millis(2),
        }
    }
}

/// Result of a [`crate::TermCore::feed`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fed {
    /// Number of leading input bytes processed. Bytes after this offset were not
    /// touched and must be fed again by the caller. Suitable as SSH window credit.
    pub consumed: usize,
}

/// Terminal dimensions in cells, plus optional cell size in pixels.
///
/// Pixel sizes are only used to answer `CSI 14 t` (text area size in pixels);
/// leave them at `0` when unknown and such queries go unanswered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    /// Number of columns (clamped to at least 2).
    pub cols: u16,
    /// Number of visible rows (clamped to at least 1).
    pub rows: u16,
    /// Cell width in pixels, `0` if unknown.
    pub cell_width: u16,
    /// Cell height in pixels, `0` if unknown.
    pub cell_height: u16,
}

impl GridSize {
    /// Size in cells with unknown pixel metrics.
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            cell_width: 0,
            cell_height: 0,
        }
    }

    /// Returns this size clamped to the minimum the emulator supports.
    pub(crate) fn clamped(self) -> Self {
        Self {
            cols: self.cols.max(2),
            rows: self.rows.max(1),
            ..self
        }
    }
}

impl Default for GridSize {
    fn default() -> Self {
        Self::new(80, 24)
    }
}

/// A position inside the visible viewport (row 0 is the top visible row).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewportPoint {
    /// Visible row, `0` at the top.
    pub row: u16,
    /// Column, `0` at the left.
    pub col: u16,
}

impl ViewportPoint {
    /// Construct a viewport point.
    pub const fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }
}

/// A position in the whole buffer.
///
/// `line` 0 is the top row of the active screen; negative lines are scrollback
/// (`-1` is the most recent history line). Coordinates shift as new history is
/// added, so treat them as valid only until the next `feed`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GridPoint {
    /// Buffer line, negative for scrollback.
    pub line: i32,
    /// Column.
    pub col: u16,
}

/// A fixed-size bitset of viewport rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowSet {
    words: Vec<u64>,
    len: usize,
}

impl RowSet {
    /// An empty set able to hold rows `0..len`.
    pub fn with_len(len: usize) -> Self {
        Self {
            words: vec![0; len.div_ceil(64)],
            len,
        }
    }

    /// Number of rows the set covers (not the number of rows set).
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the set covers zero rows.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Marks `row`; out-of-range rows are ignored.
    pub fn insert(&mut self, row: usize) {
        if row < self.len {
            self.words[row / 64] |= 1 << (row % 64);
        }
    }

    /// Marks every row.
    pub fn insert_all(&mut self) {
        for row in 0..self.len {
            self.insert(row);
        }
    }

    /// Whether `row` is marked.
    pub fn contains(&self, row: usize) -> bool {
        row < self.len && self.words[row / 64] & (1 << (row % 64)) != 0
    }

    /// Number of marked rows.
    pub fn count(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Iterates marked rows in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&row| self.contains(row))
    }
}

/// What changed since the previous [`crate::TermCore::snapshot`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Damage {
    /// Everything must be repainted (resize, clear, scroll, palette change, ...).
    /// When set, every bit in `rows` is set as well.
    pub full: bool,
    /// Rows the content moved up (positive) or down (negative) before `rows`
    /// were applied. The alacritty backend reports scrolls as full damage and
    /// always sets this to `0`.
    pub scrolled_rows: i32,
    /// Viewport rows whose cells were re-copied into the frame.
    pub rows: RowSet,
}

impl Damage {
    /// Whether nothing needs repainting.
    pub fn is_empty(&self) -> bool {
        !self.full && self.scrolled_rows == 0 && self.rows.count() == 0
    }
}

/// Bit flags stored in [`Cell16::flags`].
pub mod cell_flags {
    /// Bold weight.
    pub const BOLD: u16 = 1 << 0;
    /// Italic style.
    pub const ITALIC: u16 = 1 << 1;
    /// Faint / dim intensity.
    pub const DIM: u16 = 1 << 2;
    /// Swap foreground and background when painting.
    pub const INVERSE: u16 = 1 << 3;
    /// Concealed text: paint background only.
    pub const HIDDEN: u16 = 1 << 4;
    /// Strikethrough.
    pub const STRIKE: u16 = 1 << 5;
    /// Blinking text. Reserved: the alacritty backend does not track SGR 5/6.
    pub const BLINK: u16 = 1 << 6;
    /// First half of a double-width character.
    pub const WIDE: u16 = 1 << 7;
    /// Second half of a double-width character; paint nothing.
    pub const WIDE_SPACER: u16 = 1 << 8;
    /// The cell carries an OSC 8 hyperlink.
    pub const HYPERLINK: u16 = 1 << 9;
    /// Bit offset of the 3-bit [`super::UnderlineStyle`] field.
    pub const UNDERLINE_SHIFT: u16 = 12;
    /// Mask of the 3-bit [`super::UnderlineStyle`] field.
    pub const UNDERLINE_MASK: u16 = 0b111 << UNDERLINE_SHIFT;
}

/// Underline style encoded inside [`Cell16::flags`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum UnderlineStyle {
    /// No underline.
    None = 0,
    /// Single straight line.
    Single = 1,
    /// Double line.
    Double = 2,
    /// Curly (undercurl).
    Curly = 3,
    /// Dotted.
    Dotted = 4,
    /// Dashed.
    Dashed = 5,
}

/// Marker bit in [`Cell16::glyph`] meaning "grapheme table id" instead of a char.
pub const GRAPHEME_BIT: u32 = 0x8000_0000;

/// Marker bit in [`Cell16::ul_color`] meaning "explicit RGB555 underline colour".
pub const UL_COLOR_RGB_BIT: u16 = 0x8000;

/// A renderer-ready cell, exactly 16 bytes.
///
/// * `glyph`: Unicode scalar value, or `GRAPHEME_BIT | id` for multi-codepoint
///   graphemes resolved via [`crate::TermCore::grapheme`].
/// * `fg` / `bg`: resolved colours as `0xRRGGBBAA` (inverse is *not* applied).
/// * `flags`: see [`cell_flags`].
/// * `ul_color`: `0` = same as `fg`, otherwise `UL_COLOR_RGB_BIT | RGB555`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Cell16 {
    /// Character or grapheme id.
    pub glyph: u32,
    /// Foreground colour, `0xRRGGBBAA`.
    pub fg: u32,
    /// Background colour, `0xRRGGBBAA`.
    pub bg: u32,
    /// Style flags, see [`cell_flags`].
    pub flags: u16,
    /// Underline colour, see type docs.
    pub ul_color: u16,
}

const _: () = assert!(std::mem::size_of::<Cell16>() == 16);

impl Cell16 {
    /// The cell's character when it is a single code point.
    pub fn char(&self) -> Option<char> {
        if self.is_grapheme() {
            None
        } else {
            char::from_u32(self.glyph)
        }
    }

    /// Whether `glyph` is a grapheme table id.
    pub fn is_grapheme(&self) -> bool {
        self.glyph & GRAPHEME_BIT != 0
    }

    /// Whether every bit in `flag` is set.
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag == flag
    }

    /// Decoded underline style.
    pub fn underline(&self) -> UnderlineStyle {
        match (self.flags & cell_flags::UNDERLINE_MASK) >> cell_flags::UNDERLINE_SHIFT {
            1 => UnderlineStyle::Single,
            2 => UnderlineStyle::Double,
            3 => UnderlineStyle::Curly,
            4 => UnderlineStyle::Dotted,
            5 => UnderlineStyle::Dashed,
            _ => UnderlineStyle::None,
        }
    }

    /// Explicit underline colour as `0xRRGGBBAA`, or `None` to use `fg`.
    pub fn underline_rgba(&self) -> Option<u32> {
        if self.ul_color & UL_COLOR_RGB_BIT == 0 {
            return None;
        }
        let expand = |v: u16| -> u32 {
            let v = u32::from(v & 0x1f);
            (v << 3) | (v >> 2)
        };
        let (r, g, b) = (
            expand(self.ul_color >> 10),
            expand(self.ul_color >> 5),
            expand(self.ul_color),
        );
        Some((r << 24) | (g << 16) | (b << 8) | 0xff)
    }

    /// Packs an `0xRRGGBBAA` colour into the `ul_color` encoding.
    pub fn pack_ul_color(rgba: u32) -> u16 {
        let c = |shift: u32| ((rgba >> shift) & 0xff) as u16 >> 3;
        UL_COLOR_RGB_BIT | (c(24) << 10) | (c(16) << 5) | c(8)
    }
}

/// Cursor shapes a renderer must support.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    /// Filled block.
    #[default]
    Block,
    /// Underline bar.
    Underline,
    /// Vertical beam.
    Beam,
    /// Outlined block (e.g. unfocused).
    HollowBlock,
}

/// Cursor as of the last snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CursorState {
    /// Viewport position (meaningless when `visible` is false).
    pub pos: ViewportPoint,
    /// Requested shape.
    pub shape: CursorShape,
    /// Shown by the application (DECTCEM) and inside the viewport.
    pub visible: bool,
    /// Application requested blinking.
    pub blinking: bool,
}

/// Selected region clipped to the viewport, inclusive on both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionSpan {
    /// First selected cell.
    pub start: ViewportPoint,
    /// Last selected cell.
    pub end: ViewportPoint,
    /// Rectangular selection: only columns `start.col..=end.col` on each row.
    pub block: bool,
}

/// One visible row of a [`Frame`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowView {
    /// Exactly `Frame::size.cols` cells.
    pub cells: Vec<Cell16>,
}

/// Persistent render model filled incrementally by [`crate::TermCore::snapshot`].
///
/// Use one `Frame` per terminal: rows not reported in [`Damage`] keep the
/// contents of the previous snapshot. A frame with `generation == 0` is always fully
/// refreshed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Incremented by every snapshot.
    pub generation: u64,
    /// Grid size the rows correspond to.
    pub size: GridSize,
    /// Cursor state.
    pub cursor: CursorState,
    /// Visible rows, top to bottom.
    pub rows: Vec<RowView>,
    /// How many lines the viewport is scrolled back into history (0 = bottom).
    pub display_offset: usize,
    /// Number of lines currently held in scrollback.
    pub history_len: usize,
    /// Active selection, clipped to the viewport.
    pub selection: Option<SelectionSpan>,
}

impl Frame {
    /// Text of a visible row, with graphemes expanded through `lookup` and
    /// wide-character spacers skipped. Mostly useful for tests and debugging.
    pub fn row_text(&self, row: usize, lookup: impl Fn(u32) -> Option<String>) -> String {
        let mut out = String::new();
        if let Some(r) = self.rows.get(row) {
            for cell in &r.cells {
                if cell.has(cell_flags::WIDE_SPACER) {
                    continue;
                }
                match cell.char() {
                    Some(c) => out.push(c),
                    None => out.push_str(&lookup(cell.glyph).unwrap_or_default()),
                }
            }
        }
        out
    }
}

/// Which clipboard an OSC 52 request targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardTarget {
    /// The system clipboard (`c`).
    Clipboard,
    /// The primary selection (`p` / `s`).
    Selection,
}

/// What to do with OSC 52 clipboard *store* requests from the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardStoreMode {
    /// Drop silently.
    #[default]
    Deny,
    /// Emit [`TermEvent::ClipboardStore`].
    Allow,
    /// Emit [`TermEvent::ClipboardStoreRequest`] for the user to confirm.
    Ask,
}

/// What to do with OSC 52 clipboard *load* (read) requests from the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardLoadMode {
    /// Drop silently: the remote side never sees local clipboard contents.
    #[default]
    Deny,
    /// Emit [`TermEvent::ClipboardLoadRequest`]; reply via
    /// [`crate::TermCore::answer_clipboard_load`].
    Ask,
}

/// Policy for OSC 52 clipboard access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardPolicy {
    /// Store (copy-to-local) handling.
    pub store: ClipboardStoreMode,
    /// Load (read-local) handling.
    pub load: ClipboardLoadMode,
    /// Store requests with more decoded bytes than this are dropped.
    pub max_store_bytes: usize,
}

impl Default for ClipboardPolicy {
    fn default() -> Self {
        Self {
            store: ClipboardStoreMode::Deny,
            load: ClipboardLoadMode::Deny,
            max_store_bytes: 1024 * 1024,
        }
    }
}

/// Shell-integration marks from OSC 133.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMarkKind {
    /// `A`: prompt starts.
    PromptStart,
    /// `B`: prompt ends, command input starts.
    CommandStart,
    /// `C`: command submitted, output starts.
    CommandExecuted,
    /// `D[;exit]`: command finished.
    CommandFinished {
        /// Exit status when reported.
        exit_code: Option<i32>,
    },
}

/// Out-of-band notifications produced while feeding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    /// Window title set by OSC 0/2.
    Title(String),
    /// Title reset to the default.
    ResetTitle,
    /// BEL received.
    Bell,
    /// Working directory reported by OSC 7.
    Cwd {
        /// Host part of the `file://` URL, if any.
        host: Option<String>,
        /// Percent-decoded path.
        path: String,
    },
    /// OSC 133 shell-integration mark.
    PromptMark {
        /// Mark kind.
        kind: PromptMarkKind,
        /// Cursor row on the active screen when the mark was processed.
        cursor_line: u16,
    },
    /// Host stored text into a clipboard (policy `Allow`).
    ClipboardStore {
        /// Clipboard to write.
        target: ClipboardTarget,
        /// Decoded text.
        text: String,
    },
    /// Host wants to store text; ask the user first (policy `Ask`).
    ClipboardStoreRequest {
        /// Clipboard to write.
        target: ClipboardTarget,
        /// Decoded text.
        text: String,
    },
    /// Host wants to read a clipboard (policy `Ask`).
    ClipboardLoadRequest {
        /// Pass to [`crate::TermCore::answer_clipboard_load`].
        id: u64,
        /// Clipboard to read.
        target: ClipboardTarget,
    },
    /// Cursor shape or blinking changed (DECSCUSR).
    CursorStyle {
        /// New shape.
        shape: CursorShape,
        /// Blinking requested.
        blinking: bool,
    },
}

/// Viewport scrolling commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewportScroll {
    /// Move up into history by N lines.
    Up(u32),
    /// Move down towards the live screen by N lines.
    Down(u32),
    /// One screen up.
    PageUp,
    /// One screen down.
    PageDown,
    /// Oldest history line.
    Top,
    /// Live screen.
    Bottom,
}

/// Selection granularity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Cell by cell.
    Char,
    /// Expands to word boundaries.
    Word,
    /// Whole lines.
    Line,
    /// Rectangular block.
    Block,
}

/// Selection commands, in viewport coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOp {
    /// Begin a new selection at a cell.
    Start {
        /// Anchor cell.
        at: ViewportPoint,
        /// Granularity.
        kind: SelectionKind,
    },
    /// Move the free end of the current selection.
    Extend {
        /// New end cell.
        to: ViewportPoint,
    },
    /// Drop the selection.
    Clear,
}

/// A regex search over the visible screen and scrollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// Regular expression (regex-automata syntax).
    pub pattern: String,
    /// Match ignoring case.
    pub case_insensitive: bool,
    /// Stop after this many matches.
    pub max_matches: usize,
}

impl SearchQuery {
    /// Case-sensitive query returning at most 10 000 matches.
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_insensitive: false,
            max_matches: 10_000,
        }
    }

    /// Builder-style toggle for case-insensitive matching.
    pub fn case_insensitive(mut self, yes: bool) -> Self {
        self.case_insensitive = yes;
        self
    }
}

/// A search hit, inclusive on both ends; may span wrapped lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    /// First matched cell.
    pub start: GridPoint,
    /// Last matched cell.
    pub end: GridPoint,
}

/// Search failures.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SearchError {
    /// The pattern did not compile.
    #[error("invalid search pattern: {0}")]
    InvalidPattern(String),
}
