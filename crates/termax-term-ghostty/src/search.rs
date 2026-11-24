//! Regex search over the active screen and its history.
//!
//! libghostty-vt 0.2 has no search API, so rows are read through [`Grid`] and
//! matched with the `regex` crate, following the alacritty backend's rules:
//! soft-wrapped rows form one logical line, matches never cross a hard line
//! break, blank cells read as spaces, empty matches are skipped, and a match
//! that starts or ends on a wide character covers both of its cells.

use regex::{Regex, RegexBuilder};
use termax_term::{GridPoint, Match, SearchError, SearchQuery};
use termax_term_ghostty_sys::vt::screen::CellWide;

use crate::grid::{Grid, Pos};

/// Compiles `query` with an explicit case mode (alacritty's smart case does
/// not apply because the query says which one it wants).
pub(crate) fn compile(query: &SearchQuery) -> Result<Regex, SearchError> {
    RegexBuilder::new(&query.pattern)
        .case_insensitive(query.case_insensitive)
        .build()
        .map_err(|e| SearchError::InvalidPattern(e.to_string()))
}

/// One character of the haystack and the cells it came from.
struct Span {
    /// Byte offset in the haystack.
    offset: usize,
    /// Cell holding the character.
    first: Pos,
    /// Last cell it covers (the spacer of a wide character).
    last: Pos,
}

/// Matches of `regex` from the oldest history row down, at most `max`.
/// `history` is the number of history rows, used to convert to [`GridPoint`].
pub(crate) fn search(grid: &mut Grid<'_>, regex: &Regex, max: usize, history: u32) -> Vec<Match> {
    let mut out = Vec::new();
    let mut haystack = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let cols = grid.cols();
    let total = grid.total_rows();
    let to_grid = |p: Pos| GridPoint {
        line: i32::try_from(i64::from(p.y) - i64::from(history)).unwrap_or(i32::MIN),
        col: p.x,
    };
    let mut y = 0;
    while y < total && out.len() < max {
        haystack.clear();
        spans.clear();
        // Gather one logical line.
        loop {
            let row = grid.row(y);
            for (x, cell) in (0..cols).zip(&row.cells) {
                if matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead) {
                    continue;
                }
                let first = Pos::new(y, x);
                let last = if cell.wide == CellWide::Wide && x + 1 < cols {
                    Pos::new(y, x + 1)
                } else {
                    first
                };
                spans.push(Span {
                    offset: haystack.len(),
                    first,
                    last,
                });
                match &row.text[cell.start..cell.end] {
                    "" => haystack.push(' '),
                    text => haystack.push_str(text),
                }
            }
            let wrapped = row.wrapped;
            y += 1;
            if !wrapped || y >= total {
                break;
            }
        }
        let span_at = |byte: usize| {
            let i = spans.partition_point(|s| s.offset <= byte);
            &spans[i.saturating_sub(1)]
        };
        for m in regex.find_iter(&haystack) {
            if m.is_empty() {
                continue;
            }
            out.push(Match {
                start: to_grid(span_at(m.start()).first),
                end: to_grid(span_at(m.end() - 1).last),
            });
            if out.len() >= max {
                break;
            }
        }
    }
    out
}
