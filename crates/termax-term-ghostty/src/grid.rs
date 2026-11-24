//! Cell-level read access to the active screen (history plus active area),
//! shared by selection and search.

use termax_term_ghostty_sys::vt::screen::CellWide;
use termax_term_ghostty_sys::{RowText, Vt};

/// A cell on the active screen. `y` counts from the oldest history row, so
/// ordering is reading order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Pos {
    /// Screen row, `0` is the oldest history row.
    pub y: u32,
    /// Column.
    pub x: u16,
}

impl Pos {
    pub(crate) const fn new(y: u32, x: u16) -> Self {
        Self { y, x }
    }
}

/// What selection and search need to know about one cell, in alacritty's terms:
/// a blank cell reads as a space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellInfo {
    /// First character of the cell's text, `' '` when blank.
    pub ch: char,
    /// Width class.
    pub wide: CellWide,
}

impl CellInfo {
    /// Wide-character spacers (tail or wrapped head) carry no character.
    pub(crate) fn is_spacer(self) -> bool {
        matches!(self.wide, CellWide::SpacerTail | CellWide::SpacerHead)
    }
}

/// Rows kept around; selection and search walk rows sequentially or touch
/// neighbours, so a handful is enough.
const CACHE_ROWS: usize = 4;

/// Reads rows through grid references, caching the last few.
pub(crate) struct Grid<'a> {
    vt: &'a Vt,
    cols: u16,
    total: u32,
    cache: Vec<(u32, RowText)>,
    next_slot: usize,
}

impl<'a> Grid<'a> {
    /// A reader over the active screen; `None` if Ghostty cannot report its size.
    pub(crate) fn new(vt: &'a Vt) -> Option<Self> {
        let cols = vt.cols().ok()?;
        let total = u32::try_from(vt.total_rows().ok()?).ok()?;
        (cols > 0 && total > 0).then(|| Self {
            vt,
            cols,
            total,
            cache: Vec::with_capacity(CACHE_ROWS),
            next_slot: 0,
        })
    }

    pub(crate) fn cols(&self) -> u16 {
        self.cols
    }

    pub(crate) fn last_col(&self) -> u16 {
        self.cols - 1
    }

    /// Rows on the active screen, history included.
    pub(crate) fn total_rows(&self) -> u32 {
        self.total
    }

    /// Row `y`, read on a cache miss. A row Ghostty fails to resolve reads as
    /// blank rather than aborting the caller.
    pub(crate) fn row(&mut self, y: u32) -> &RowText {
        let slot = match self.cache.iter().position(|(row, _)| *row == y) {
            Some(slot) => slot,
            None => {
                let slot = if self.cache.len() < CACHE_ROWS {
                    self.cache.push((y, RowText::default()));
                    self.cache.len() - 1
                } else {
                    let slot = self.next_slot;
                    self.next_slot = (slot + 1) % CACHE_ROWS;
                    slot
                };
                let entry = &mut self.cache[slot];
                entry.0 = y;
                if self.vt.read_row(y, &mut entry.1).is_err() || entry.1.cells.is_empty() {
                    entry.1 = RowText::default();
                }
                slot
            }
        };
        &self.cache[slot].1
    }

    /// Whether row `y` soft-wraps onto the next row.
    pub(crate) fn wrapped(&mut self, y: u32) -> bool {
        self.row(y).wrapped
    }

    pub(crate) fn cell(&mut self, p: Pos) -> CellInfo {
        let row = self.row(p.y);
        match row.cells.get(usize::from(p.x)) {
            Some(span) => CellInfo {
                ch: row.text[span.start..span.end].chars().next().unwrap_or(' '),
                wide: span.wide,
            },
            None => CellInfo {
                ch: ' ',
                wide: CellWide::Narrow,
            },
        }
    }

    /// The following cell in reading order, `None` past the last cell.
    pub(crate) fn next(&self, p: Pos) -> Option<Pos> {
        if p.x < self.last_col() {
            Some(Pos::new(p.y, p.x + 1))
        } else if p.y + 1 < self.total {
            Some(Pos::new(p.y + 1, 0))
        } else {
            None
        }
    }

    /// The preceding cell in reading order, `None` before the first cell.
    pub(crate) fn prev(&self, p: Pos) -> Option<Pos> {
        if p.x > 0 {
            Some(Pos::new(p.y, p.x - 1))
        } else if p.y > 0 {
            Some(Pos::new(p.y - 1, self.last_col()))
        } else {
            None
        }
    }
}
