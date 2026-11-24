//! Selection ranges and selection text with `alacritty_terminal` semantics.
//!
//! libghostty-vt 0.2 can derive word and line selections and format them, but
//! its word boundaries, line trimming and copy format differ from the alacritty
//! backend's (for example a line selection there ends with a newline and a
//! double-click on a bracket selects up to its partner). This module ports
//! alacritty's rules (`selection.rs`, `term/search.rs`, `term/mod.rs` of
//! alacritty_terminal 0.26) over [`Grid`] so both backends select and copy
//! the same text.

use std::cmp;

use termax_term::SelectionKind;
use termax_term_ghostty_sys::RowText;
use termax_term_ghostty_sys::vt::screen::CellWide;

use crate::grid::{Grid, Pos};

/// alacritty's default `semantic_escape_chars`.
const SEMANTIC_ESCAPE_CHARS: &str = ",│`|:\"' ()[]{}<>\t";

/// Pairs matched when a word selection starts on a bracket.
const BRACKET_PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// Which half of a cell a selection endpoint sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Anchor {
    point: Pos,
    side: Side,
}

/// A resolved selection, inclusive on both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    pub start: Pos,
    pub end: Pos,
    /// Rectangular: only columns `start.x..=end.x` on each row.
    pub block: bool,
}

/// Resolves a selection started at `anchor` and, once extended, ending at
/// `target`. `None` means empty (a char or block selection not yet dragged).
///
/// Mirrors the backend's `Selection` handling in alacritty: the anchor is the
/// left side of its cell, and the free end is the right side of its cell when
/// it lies at or after the anchor, the left side otherwise.
pub(crate) fn resolve(
    grid: &mut Grid<'_>,
    kind: SelectionKind,
    anchor: Pos,
    target: Option<Pos>,
) -> Option<Range> {
    let start = Anchor {
        point: anchor,
        side: Side::Left,
    };
    let end = match target {
        None => start,
        Some(point) => Anchor {
            point,
            side: if point < anchor {
                Side::Left
            } else {
                Side::Right
            },
        },
    };
    let (first, last) = if start.point > end.point {
        (end, start)
    } else {
        (start, end)
    };
    match kind {
        SelectionKind::Char => range_simple(first, last, grid.cols()),
        SelectionKind::Block => range_block(start, end, first, last),
        SelectionKind::Word => Some(range_semantic(grid, first.point, last.point)),
        SelectionKind::Line => Some(Range {
            start: line_search_left(grid, first.point),
            end: line_search_right(grid, last.point),
            block: false,
        }),
    }
}

fn range_simple(mut start: Anchor, mut end: Anchor, cols: u16) -> Option<Range> {
    let empty = start == end
        || (start.side == Side::Right
            && end.side == Side::Left
            && start.point.y == end.point.y
            && start.point.x + 1 == end.point.x);
    if empty {
        return None;
    }
    // Drop the last cell when the selection ends on its left half.
    if end.side == Side::Left && start.point != end.point {
        if end.point.x == 0 {
            end.point = Pos::new(end.point.y.checked_sub(1)?, cols - 1);
        } else {
            end.point.x -= 1;
        }
    }
    // Drop the first cell when the selection starts on its right half.
    if start.side == Side::Right && start.point != end.point {
        start.point.x += 1;
        if start.point.x == cols {
            start.point = Pos::new(start.point.y + 1, 0);
        }
    }
    Some(Range {
        start: start.point,
        end: end.point,
        block: false,
    })
}

/// `region_start`/`region_end` are the endpoints as created (emptiness is
/// judged on them); `start`/`end` are the same endpoints in reading order.
fn range_block(
    region_start: Anchor,
    region_end: Anchor,
    mut start: Anchor,
    mut end: Anchor,
) -> Option<Range> {
    let (s, e) = (region_start, region_end);
    let empty = (s.point.x == e.point.x && s.side == e.side)
        || (s.point.x + 1 == e.point.x && s.side == Side::Right && e.side == Side::Left)
        || (e.point.x + 1 == s.point.x && s.side == Side::Left && e.side == Side::Right);
    if empty {
        return None;
    }
    // Always go top-left to bottom-right.
    if start.point.x > end.point.x {
        std::mem::swap(&mut start.side, &mut end.side);
        std::mem::swap(&mut start.point.x, &mut end.point.x);
    }
    if end.side == Side::Left && start.point != end.point && end.point.x > 0 {
        end.point.x -= 1;
    }
    if start.side == Side::Right && start.point != end.point {
        start.point.x += 1;
    }
    Some(Range {
        start: start.point,
        end: end.point,
        block: true,
    })
}

fn range_semantic(grid: &mut Grid<'_>, start: Pos, end: Pos) -> Range {
    if start == end
        && let Some(matching) = bracket_search(grid, start)
    {
        let (start, end) = if matching < start {
            (matching, start)
        } else {
            (start, matching)
        };
        return Range {
            start,
            end,
            block: false,
        };
    }
    Range {
        start: semantic_search_left(grid, start),
        end: semantic_search_right(grid, end),
        block: false,
    }
}

/// The bracket matching the one at `point`, honouring nesting; scans the
/// whole screen like alacritty does.
fn bracket_search(grid: &mut Grid<'_>, point: Pos) -> Option<Pos> {
    let start_char = grid.cell(point).ch;
    let (forward, end_char) = BRACKET_PAIRS.iter().find_map(|&(open, close)| {
        if open == start_char {
            Some((true, close))
        } else if close == start_char {
            Some((false, open))
        } else {
            None
        }
    })?;
    let mut skip_pairs = 0usize;
    let mut cur = point;
    loop {
        cur = if forward {
            grid.next(cur)
        } else {
            grid.prev(cur)
        }?;
        let c = grid.cell(cur).ch;
        if c == end_char && skip_pairs == 0 {
            return Some(cur);
        } else if c == start_char {
            skip_pairs += 1;
        } else if c == end_char {
            skip_pairs -= 1;
        }
    }
}

fn is_escape(c: char) -> bool {
    SEMANTIC_ESCAPE_CHARS.contains(c)
}

/// Walks left from `point` within its logical line until an escape character.
/// `Ok` is the escape character's cell, `Err` the last cell reached.
fn inline_search_left(grid: &mut Grid<'_>, mut point: Pos) -> Result<Pos, Pos> {
    let last_col = grid.last_col();
    let mut cur = point;
    while let Some(prev) = grid.prev(cur) {
        if prev.x == last_col && !grid.wrapped(prev.y) {
            break;
        }
        cur = prev;
        point = prev;
        let cell = grid.cell(prev);
        if !cell.is_spacer() && is_escape(cell.ch) {
            return Ok(point);
        }
    }
    Err(point)
}

fn inline_search_right(grid: &mut Grid<'_>, mut point: Pos) -> Result<Pos, Pos> {
    let last_col = grid.last_col();
    if point.x == last_col && !grid.wrapped(point.y) {
        return Err(point);
    }
    let mut cur = point;
    while let Some(next) = grid.next(cur) {
        cur = next;
        point = next;
        let cell = grid.cell(next);
        if !cell.is_spacer() && is_escape(cell.ch) {
            return Ok(point);
        }
        if next.x == last_col && !grid.wrapped(next.y) {
            break;
        }
    }
    Err(point)
}

fn semantic_search_left(grid: &mut Grid<'_>, point: Pos) -> Pos {
    match inline_search_left(grid, point) {
        // Step back right past the escape character, skipping wide spacers.
        Ok(found) => {
            let mut cur = found;
            while let Some(next) = grid.next(cur) {
                if !grid.cell(next).is_spacer() {
                    return next;
                }
                cur = next;
            }
            found
        }
        Err(point) => point,
    }
}

fn semantic_search_right(grid: &mut Grid<'_>, point: Pos) -> Pos {
    match inline_search_right(grid, point) {
        Ok(found) => grid.prev(found).unwrap_or(found),
        Err(point) => point,
    }
}

fn line_search_left(grid: &mut Grid<'_>, mut point: Pos) -> Pos {
    while point.y > 0 && grid.wrapped(point.y - 1) {
        point.y -= 1;
    }
    point.x = 0;
    point
}

fn line_search_right(grid: &mut Grid<'_>, mut point: Pos) -> Pos {
    while point.y + 1 < grid.total_rows() && grid.wrapped(point.y) {
        point.y += 1;
    }
    point.x = grid.last_col();
    point
}

/// Copy text of `range`, formatted like alacritty's `selection_to_string`.
pub(crate) fn text(grid: &mut Grid<'_>, kind: SelectionKind, range: Range) -> String {
    let Range { start, end, .. } = range;
    match kind {
        SelectionKind::Block => {
            let mut res = String::new();
            for y in start.y..end.y {
                res += line_to_string(grid, y, start.x, end.x, start.x != 0).trim_end();
                res.push('\n');
            }
            res += line_to_string(grid, end.y, start.x, end.x, true).trim_end();
            res
        }
        SelectionKind::Line => bounds_to_string(grid, start, end) + "\n",
        SelectionKind::Char | SelectionKind::Word => bounds_to_string(grid, start, end),
    }
}

fn bounds_to_string(grid: &mut Grid<'_>, start: Pos, end: Pos) -> String {
    let mut res = String::new();
    for y in start.y..=end.y {
        let start_x = if y == start.y { start.x } else { 0 };
        let end_x = if y == end.y { end.x } else { grid.last_col() };
        res += &line_to_string(grid, y, start_x, end_x, y == end.y);
    }
    match res.strip_suffix('\n') {
        Some(stripped) => stripped.to_owned(),
        None => res,
    }
}

/// Columns up to and including the last non-blank cell (all of them when the
/// row wraps).
fn line_length(row: &RowText, cols: usize) -> usize {
    if row.wrapped {
        return cols;
    }
    (0..row.cells.len())
        .rev()
        .find(|&x| !matches!(row.cell_text(x), "" | " "))
        .map_or(0, |x| x + 1)
}

/// Text of columns `start_x..=end_x` of row `y`. Trailing blanks are dropped
/// and a newline is added when the selection reaches the end of a row that
/// does not wrap.
///
/// alacritty additionally collapses the blanks after a stored tab character;
/// Ghostty does not store tabs in cells, so there is nothing to collapse.
fn line_to_string(
    grid: &mut Grid<'_>,
    y: u32,
    start_x: u16,
    end_x: u16,
    include_wrapped_wide: bool,
) -> String {
    let cols = usize::from(grid.cols());
    let row = grid.row(y);
    let mut text = String::new();
    let line_length = cmp::min(line_length(row, cols), usize::from(end_x) + 1);
    let mut start_x = usize::from(start_x);
    // Include the wide character when only its trailing spacer is selected.
    if start_x > 0 && row.cells.get(start_x).map(|c| c.wide) == Some(CellWide::SpacerTail) {
        start_x -= 1;
    }
    for x in start_x..line_length {
        let Some(cell) = row.cells.get(x) else {
            break;
        };
        if matches!(cell.wide, CellWide::SpacerTail | CellWide::SpacerHead) {
            continue;
        }
        match row.cell_text(x) {
            "" => text.push(' '),
            cell_text => text.push_str(cell_text),
        }
    }
    let wrapped = row.wrapped;
    let ends_with_head = line_length == cols
        && cols >= 2
        && row.cells.get(cols - 1).map(|c| c.wide) == Some(CellWide::SpacerHead);
    if usize::from(end_x) + 1 >= cols && !wrapped {
        text.push('\n');
    }
    // The row ends in the padding before a wide character that wrapped: copy
    // that character too. (alacritty 0.26 reads the row above here, which
    // looks like a slip; the wrapped character starts the next row.)
    if ends_with_head && include_wrapped_wide && y + 1 < grid.total_rows() {
        text.push(grid.cell(Pos::new(y + 1, 0)).ch);
    }
    text
}
