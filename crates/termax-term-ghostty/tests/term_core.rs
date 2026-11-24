//! Parity subset of `termax-term`'s behavioural tests for the Ghostty backend.
//!
//! Assertions are kept identical to the alacritty suite wherever the backend
//! supports the behaviour; deliberate differences are commented in place.

use std::time::Duration;

use termax_term::{
    Budget, ClipboardLoadMode, ClipboardPolicy, ClipboardStoreMode, ClipboardTarget, Frame,
    GridSize, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind, PastePolicy,
    PromptMarkKind, SearchQuery, SelectionKind, SelectionOp, TermConfig, TermCore, TermEvent,
    UnderlineStyle, ViewportPoint, ViewportScroll, cell_flags,
};
use termax_term_ghostty::GhosttyTerm;

fn term(cols: u16, rows: u16) -> GhosttyTerm {
    GhosttyTerm::new(TermConfig {
        size: GridSize::new(cols, rows),
        ..Default::default()
    })
}

fn term_without_scrollback(cols: u16, rows: u16) -> GhosttyTerm {
    GhosttyTerm::new(TermConfig {
        size: GridSize::new(cols, rows),
        scrollback_lines: 0,
        ..Default::default()
    })
}

fn term_with_clipboard(clipboard: ClipboardPolicy) -> GhosttyTerm {
    GhosttyTerm::new(TermConfig {
        clipboard,
        ..Default::default()
    })
}

fn feed_all(t: &mut GhosttyTerm, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let fed = t.feed(rest, Budget::UNLIMITED);
        assert!(fed.consumed > 0, "feed must make progress");
        rest = &rest[fed.consumed..];
    }
}

fn events(t: &mut GhosttyTerm) -> Vec<TermEvent> {
    let mut out = Vec::new();
    t.take_events(&mut out);
    out
}

fn replies(t: &mut GhosttyTerm) -> Vec<u8> {
    let mut out = Vec::new();
    t.drain_replies(&mut out);
    out
}

fn row_text(t: &GhosttyTerm, frame: &Frame, row: usize) -> String {
    frame
        .row_text(row, |g| t.grapheme(g).map(str::to_owned))
        .trim_end()
        .to_owned()
}

fn numbered_lines(from: usize, to: usize) -> Vec<u8> {
    (from..to)
        .map(|i| format!("line {i}\r\n"))
        .collect::<String>()
        .into_bytes()
}

#[test]
fn trait_is_object_safe_and_send() {
    fn assert_send<T: Send + 'static>() {}
    assert_send::<GhosttyTerm>();
    let mut boxed: Box<dyn TermCore> = Box::new(term(10, 2));
    let fed = boxed.feed(b"hi", Budget::default());
    assert_eq!(fed.consumed, 2);
}

#[test]
fn feed_honours_byte_budget() {
    let mut t = term(80, 24);
    let data = vec![b'x'; 10_000];
    let fed = t.feed(
        &data,
        Budget {
            max_bytes: 1_000,
            max_time: Duration::from_secs(1),
        },
    );
    assert_eq!(fed.consumed, 1_000);
    let fed = t.feed(
        &data[1_000..],
        Budget {
            max_bytes: 64 * 1024,
            max_time: Duration::from_secs(5),
        },
    );
    assert_eq!(fed.consumed, 9_000);
}

#[test]
fn feed_honours_time_budget_between_chunks() {
    let mut t = term(80, 24);
    let data = vec![b'x'; 64 * 1024];
    let fed = t.feed(
        &data,
        Budget {
            max_bytes: usize::MAX,
            max_time: Duration::ZERO,
        },
    );
    // At least one chunk always runs; a zero time budget stops right after it.
    assert!(
        fed.consumed > 0 && fed.consumed < data.len(),
        "consumed {}",
        fed.consumed
    );
}

#[test]
fn split_utf8_and_split_escape_across_feeds() {
    let mut t = term(20, 3);
    let input = "\x1b[31mỜ\x1b[0mz".as_bytes();
    for b in input {
        feed_all(&mut t, std::slice::from_ref(b));
    }
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "Ờz");
    assert_eq!(frame.rows[0].cells[0].fg, 0xcd0000ff);
    assert_ne!(frame.rows[0].cells[1].fg, 0xcd0000ff);
}

#[test]
fn device_attribute_and_status_replies() {
    let mut t = term(20, 5);
    feed_all(&mut t, b"\x1b[c");
    assert_eq!(replies(&mut t), b"\x1b[?6c");
    feed_all(&mut t, b"\x1b[3;7H\x1b[6n");
    assert_eq!(replies(&mut t), b"\x1b[3;7R");
    feed_all(&mut t, b"\x1b[5n");
    assert_eq!(replies(&mut t), b"\x1b[0n");
}

#[test]
fn title_keeps_vietnamese_with_0x9c_byte() {
    // U+1EDC is E1 BB 9C in UTF-8; 0x9C is also the C1 string terminator.
    assert!("Ờ".as_bytes().contains(&0x9c));
    let mut t = term(20, 5);
    feed_all(&mut t, "\x1b]2;Xin chào Ờ đây\x07".as_bytes());
    feed_all(&mut t, "\x1b]0;Ờ\x1b\\".as_bytes());
    assert_eq!(
        events(&mut t),
        vec![
            TermEvent::Title("Xin chào Ờ đây".into()),
            TermEvent::Title("Ờ".into())
        ]
    );
}

#[test]
fn bell_event() {
    let mut t = term(20, 5);
    feed_all(&mut t, b"a\x07b");
    assert_eq!(events(&mut t), vec![TermEvent::Bell]);
}

#[test]
fn osc7_and_osc133_split_across_feeds() {
    let mut t = term(20, 5);
    let input = b"\x1b]7;file://host/tmp/a%20b\x1b\\\r\n\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
    for chunk in input.chunks(3) {
        feed_all(&mut t, chunk);
    }
    assert_eq!(
        events(&mut t),
        vec![
            TermEvent::Cwd {
                host: Some("host".into()),
                path: "/tmp/a b".into()
            },
            TermEvent::PromptMark {
                kind: PromptMarkKind::PromptStart,
                cursor_line: 1
            },
            TermEvent::PromptMark {
                kind: PromptMarkKind::CommandStart,
                cursor_line: 1
            },
            TermEvent::PromptMark {
                kind: PromptMarkKind::CommandExecuted,
                cursor_line: 2
            },
            TermEvent::PromptMark {
                kind: PromptMarkKind::CommandFinished { exit_code: Some(0) },
                cursor_line: 3
            },
        ]
    );
    // The marks are invisible: only the text reached the grid.
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 1), "$ ls");
    assert_eq!(row_text(&t, &frame, 2), "out");
}

#[test]
fn color_query_is_answered_from_palette() {
    let mut t = term(20, 3);
    feed_all(&mut t, b"\x1b]11;?\x07");
    assert_eq!(replies(&mut t), b"\x1b]11;rgb:0000/0000/0000\x07");
    feed_all(&mut t, b"\x1b]4;1;?\x07");
    assert_eq!(replies(&mut t), b"\x1b]4;1;rgb:cdcd/0000/0000\x07");
}

#[test]
fn clipboard_policy_gates_osc52() {
    // "hello" in base64.
    let store = b"\x1b]52;c;aGVsbG8=\x07";
    let load = b"\x1b]52;c;?\x07";

    let mut deny = term(20, 5);
    feed_all(&mut deny, store);
    feed_all(&mut deny, load);
    assert!(events(&mut deny).is_empty());
    assert!(replies(&mut deny).is_empty());

    let mut allow = GhosttyTerm::new(TermConfig {
        clipboard: ClipboardPolicy {
            store: ClipboardStoreMode::Allow,
            ..Default::default()
        },
        ..Default::default()
    });
    feed_all(&mut allow, store);
    feed_all(&mut allow, load);
    assert_eq!(
        events(&mut allow),
        vec![TermEvent::ClipboardStore {
            target: ClipboardTarget::Clipboard,
            text: "hello".into()
        }]
    );

    let mut ask = GhosttyTerm::new(TermConfig {
        clipboard: ClipboardPolicy {
            store: ClipboardStoreMode::Ask,
            load: ClipboardLoadMode::Ask,
            ..Default::default()
        },
        ..Default::default()
    });
    feed_all(&mut ask, store);
    feed_all(&mut ask, load);
    let evs = events(&mut ask);
    assert_eq!(
        evs[0],
        TermEvent::ClipboardStoreRequest {
            target: ClipboardTarget::Clipboard,
            text: "hello".into()
        }
    );
    let TermEvent::ClipboardLoadRequest {
        id,
        target: ClipboardTarget::Clipboard,
    } = evs[1]
    else {
        panic!("expected load request, got {evs:?}");
    };
    ask.answer_clipboard_load(id, Some("hi"));
    assert_eq!(replies(&mut ask), b"\x1b]52;c;aGk=\x07");
    // A second answer for the same id is ignored.
    ask.answer_clipboard_load(id, Some("again"));
    assert!(replies(&mut ask).is_empty());

    let mut small = GhosttyTerm::new(TermConfig {
        clipboard: ClipboardPolicy {
            store: ClipboardStoreMode::Allow,
            max_store_bytes: 3,
            ..Default::default()
        },
        ..Default::default()
    });
    feed_all(&mut small, store);
    assert!(events(&mut small).is_empty());
}

#[test]
fn damage_after_one_line_touches_only_that_row() {
    let mut t = term(20, 6);
    let mut frame = Frame::default();
    let first = t.snapshot(&mut frame);
    assert!(first.full);
    assert_eq!(frame.generation, 1);

    feed_all(&mut t, b"\x1b[3;1Hhello");
    // Moving the cursor damages its old position; settle that first.
    t.snapshot(&mut frame);
    feed_all(&mut t, b" world");
    let damage = t.snapshot(&mut frame);
    assert!(!damage.full);
    assert_eq!(damage.rows.iter().collect::<Vec<_>>(), vec![2]);
    assert_eq!(row_text(&t, &frame, 2), "hello world");
    assert_eq!(frame.generation, 3);

    // Unlike alacritty, Ghostty does not re-damage the cursor row when
    // nothing changed: an idle snapshot is empty.
    let idle = t.snapshot(&mut frame);
    assert!(idle.is_empty(), "{idle:?}");
}

#[test]
fn full_damage_after_resize_and_clear() {
    let mut t = term(20, 6);
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    t.resize(GridSize::new(30, 8));
    let d = t.snapshot(&mut frame);
    assert!(d.full);
    assert_eq!(d.rows.count(), 8);
    assert_eq!(frame.rows.len(), 8);
    assert!(frame.rows.iter().all(|r| r.cells.len() == 30));

    let d = t.snapshot(&mut frame);
    assert!(!d.full);
    feed_all(&mut t, b"\x1b[2J");
    assert!(t.snapshot(&mut frame).full);
}

#[test]
fn sgr_colors_resolve_to_rgba() {
    let mut t = term(20, 3);
    feed_all(
        &mut t,
        b"\x1b[31mA\x1b[38;5;196mB\x1b[38;2;1;2;3;48;5;21mC\x1b[0mD",
    );
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    let cells = &frame.rows[0].cells;
    assert_eq!(cells[0].fg, 0xcd0000ff);
    assert_eq!(cells[1].fg, 0xff0000ff);
    assert_eq!(cells[2].fg, 0x010203ff);
    assert_eq!(cells[2].bg, 0x0000ffff);
    assert_eq!(cells[3].fg, 0xe5e5e5ff, "default foreground");
    assert_eq!(cells[3].bg, 0x000000ff, "default background");
}

#[test]
fn attributes_map_to_flags() {
    let mut t = term(20, 3);
    feed_all(
        &mut t,
        b"\x1b[1;3;9mA\x1b[0;4:3;58;2;255;0;0mB\x1b[0;4:2mC\x1b[0;2;7;8mD\x1b[0;4:4mE\x1b[0;4:5mF",
    );
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    let c = &frame.rows[0].cells;
    assert!(c[0].has(cell_flags::BOLD | cell_flags::ITALIC | cell_flags::STRIKE));
    assert_eq!(c[1].underline(), UnderlineStyle::Curly);
    assert_eq!(c[1].underline_rgba(), Some(0xff0000ff));
    assert_eq!(c[2].underline(), UnderlineStyle::Double);
    assert!(c[3].has(cell_flags::DIM | cell_flags::INVERSE | cell_flags::HIDDEN));
    assert_eq!(c[4].underline(), UnderlineStyle::Dotted);
    assert_eq!(c[5].underline(), UnderlineStyle::Dashed);
    assert_eq!(c[0].underline(), UnderlineStyle::None);
    assert_eq!(c[0].underline_rgba(), None);
}

#[test]
fn wide_chars_set_wide_and_spacer_flags() {
    let mut t = term(20, 3);
    feed_all(&mut t, "中😀a".as_bytes());
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    let c = &frame.rows[0].cells;
    assert_eq!(c[0].char(), Some('中'));
    assert!(c[0].has(cell_flags::WIDE));
    assert!(c[1].has(cell_flags::WIDE_SPACER));
    assert_eq!(c[2].char(), Some('😀'));
    assert!(c[2].has(cell_flags::WIDE));
    assert!(c[3].has(cell_flags::WIDE_SPACER));
    assert_eq!(c[4].char(), Some('a'));
    assert_eq!(frame.cursor.pos, ViewportPoint::new(0, 5));
    assert_eq!(row_text(&t, &frame, 0), "中😀a");
}

#[test]
fn combining_marks_use_grapheme_table() {
    let mut t = term(20, 3);
    feed_all(&mut t, "e\u{301}x".as_bytes());
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    let c = &frame.rows[0].cells;
    assert!(c[0].is_grapheme());
    assert_eq!(c[0].char(), None);
    assert_eq!(t.grapheme(c[0].glyph), Some("e\u{301}"));
    assert_eq!(c[1].char(), Some('x'));
    assert_eq!(t.grapheme(u32::from('x')), None);
}

#[test]
fn arrow_keys_follow_decckm() {
    let mut t = term(20, 3);
    let up = KeyEvent::plain(Key::Up);
    assert_eq!(t.encode_key(&up).unwrap(), b"\x1b[A");
    feed_all(&mut t, b"\x1b[?1h");
    assert!(t.input_modes().app_cursor);
    assert_eq!(t.encode_key(&up).unwrap(), b"\x1bOA");
    assert_eq!(
        t.encode_key(&KeyEvent::new(Key::Up, Modifiers::CTRL))
            .unwrap(),
        b"\x1b[1;5A"
    );
    feed_all(&mut t, b"\x1b[?1l");
    assert_eq!(t.encode_key(&up).unwrap(), b"\x1b[A");
}

#[test]
fn paste_follows_bracketed_mode() {
    let mut t = term(20, 3);
    let policy = PastePolicy {
        allow_escape: true,
        ..Default::default()
    };
    let plain = t.encode_paste("a\nb", &policy);
    assert_eq!(plain.bytes, b"a\rb");
    assert!(plain.needs_confirmation);

    feed_all(&mut t, b"\x1b[?2004h");
    let out = t.encode_paste("safe\x1b[201~evil", &policy);
    assert_eq!(out.bytes, b"\x1b[200~safeevil\x1b[201~");
    assert!(!out.needs_confirmation);
}

#[test]
fn mouse_reporting_follows_modes() {
    let mut t = term(80, 24);
    let press = MouseEvent {
        kind: MouseEventKind::Press(MouseButton::Left),
        at: ViewportPoint::new(2, 3),
        mods: Modifiers::NONE,
    };
    assert!(t.encode_mouse(&press).is_none());
    feed_all(&mut t, b"\x1b[?1000h");
    assert_eq!(
        t.encode_mouse(&press).unwrap(),
        [0x1b, b'[', b'M', 32, 36, 35]
    );
    feed_all(&mut t, b"\x1b[?1006h");
    assert_eq!(t.encode_mouse(&press).unwrap(), b"\x1b[<0;4;3M");
    feed_all(&mut t, b"\x1b[?1000l");
    assert!(t.encode_mouse(&press).is_none());
}

#[test]
fn focus_reports_only_in_mode_1004() {
    let mut t = term(20, 3);
    assert!(t.encode_focus(true).is_none());
    feed_all(&mut t, b"\x1b[?1004h");
    assert_eq!(t.encode_focus(true).unwrap(), b"\x1b[I");
    assert_eq!(t.encode_focus(false).unwrap(), b"\x1b[O");
}

#[test]
fn scrollback_viewport_stays_anchored() {
    let mut t = term(20, 5);
    feed_all(&mut t, &numbered_lines(0, 30));
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(frame.history_len, 26);

    t.scroll(ViewportScroll::Up(3));
    t.snapshot(&mut frame);
    assert_eq!(frame.display_offset, 3);
    let top_before = row_text(&t, &frame, 0);
    assert_eq!(top_before, "line 23");
    assert!(!frame.cursor.visible, "cursor row is below the viewport");

    feed_all(&mut t, &numbered_lines(30, 32));
    t.snapshot(&mut frame);
    assert_eq!(frame.display_offset, 5);
    assert_eq!(row_text(&t, &frame, 0), top_before);

    t.scroll(ViewportScroll::Bottom);
    t.snapshot(&mut frame);
    assert_eq!(frame.display_offset, 0);
    assert_eq!(row_text(&t, &frame, 3), "line 31");
    assert!(frame.cursor.visible);

    t.scroll(ViewportScroll::Top);
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "line 0");
    t.scroll(ViewportScroll::PageDown);
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "line 5");
}

#[test]
fn scrollback_keeps_at_least_the_configured_lines() {
    // Ghostty's limit is a byte budget rounded up to whole pages, so the
    // exact cap of the alacritty test (10 lines) cannot be reproduced; check
    // the configured history survives and that it is bounded.
    let mut t = GhosttyTerm::new(TermConfig {
        size: GridSize::new(80, 24),
        scrollback_lines: 1_000,
        ..Default::default()
    });
    feed_all(&mut t, &numbered_lines(0, 20_000));
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert!(
        (1_000..20_000).contains(&frame.history_len),
        "history {}",
        frame.history_len
    );
}

#[test]
fn resize_reflows_wrapped_lines() {
    let mut t = term(10, 4);
    feed_all(&mut t, b"abcdefghijKLM");
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "abcdefghij");
    assert_eq!(row_text(&t, &frame, 1), "KLM");
    t.resize(GridSize::new(20, 4));
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "abcdefghijKLM");
}

#[test]
fn cursor_shape_and_visibility() {
    let mut t = term(20, 3);
    feed_all(&mut t, b"\x1b[5 q\x1b[?25l");
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert!(!frame.cursor.visible);
    assert_eq!(frame.cursor.shape, termax_term::CursorShape::Beam);
    // Raised at snapshot time on this backend (see crate docs).
    assert!(events(&mut t).contains(&TermEvent::CursorStyle {
        shape: termax_term::CursorShape::Beam,
        blinking: true
    }));
}

#[test]
fn search_finds_text_scrolled_off_screen() {
    let mut t = term(20, 5);
    feed_all(&mut t, b"the Needle here\r\n");
    feed_all(&mut t, &numbered_lines(0, 20));
    let hits = t.search(&SearchQuery::new("needle")).unwrap();
    assert!(hits.is_empty(), "case-sensitive by default");

    let hits = t
        .search(&SearchQuery::new("needle").case_insensitive(true))
        .unwrap();
    assert_eq!(hits.len(), 1);
    let m = hits[0];
    assert!(m.start.line < 0, "match is in scrollback: {m:?}");
    assert_eq!((m.start.col, m.end.col), (4, 9));
    assert_eq!(m.start.line, m.end.line);

    let hits = t.search(&SearchQuery::new(r"line 1\d")).unwrap();
    assert_eq!(hits.len(), 10);
    assert!(hits.windows(2).all(|w| w[0].start < w[1].start));

    let mut capped = SearchQuery::new("line");
    capped.max_matches = 3;
    assert_eq!(t.search(&capped).unwrap().len(), 3);

    assert!(t.search(&SearchQuery::new("(")).is_err());
}

#[test]
fn selection_text_char_word_line_block() {
    let mut t = term(20, 4);
    feed_all(&mut t, b"hello brave world\r\nsecond line here");

    t.select(SelectionOp::Start {
        at: ViewportPoint::new(0, 6),
        kind: SelectionKind::Word,
    });
    assert_eq!(t.selection_text().as_deref(), Some("brave"));

    t.select(SelectionOp::Start {
        at: ViewportPoint::new(0, 0),
        kind: SelectionKind::Char,
    });
    t.select(SelectionOp::Extend {
        to: ViewportPoint::new(0, 4),
    });
    assert_eq!(t.selection_text().as_deref(), Some("hello"));

    t.select(SelectionOp::Start {
        at: ViewportPoint::new(1, 3),
        kind: SelectionKind::Line,
    });
    assert_eq!(t.selection_text().as_deref(), Some("second line here\n"));

    t.select(SelectionOp::Start {
        at: ViewportPoint::new(0, 0),
        kind: SelectionKind::Block,
    });
    t.select(SelectionOp::Extend {
        to: ViewportPoint::new(1, 2),
    });
    assert_eq!(t.selection_text().as_deref(), Some("hel\nsec"));

    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    let span = frame.selection.expect("selection span");
    assert!(span.block);
    assert_eq!(span.start, ViewportPoint::new(0, 0));
    assert_eq!(span.end, ViewportPoint::new(1, 2));

    t.select(SelectionOp::Clear);
    assert_eq!(t.selection_text(), None);
    assert!(t.snapshot(&mut frame).full);
    assert_eq!(frame.selection, None);
}

/// Small deterministic generator so the test needs no extra dependency.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn one_megabyte_of_random_bytes_does_not_panic() {
    let mut t = term(80, 24);
    let mut seed = 0x9e37_79b9_7f4a_7c15;
    let mut data = Vec::with_capacity(1 << 20);
    while data.len() < 1 << 20 {
        let r = xorshift(&mut seed);
        // Bias towards ESC and sequence bytes so the parser hits many states.
        let byte = match r % 8 {
            0 => 0x1b,
            1 => b"[]P;?0123456789mhlHJK\x07\x9c"[(r >> 8) as usize % 23],
            _ => (r >> 16) as u8,
        };
        data.push(byte);
    }
    let mut frame = Frame::default();
    let mut rest = &data[..];
    while !rest.is_empty() {
        let fed = t.feed(rest, Budget::default());
        assert!(fed.consumed > 0);
        rest = &rest[fed.consumed..];
        t.snapshot(&mut frame);
        let _ = t.encode_key(&KeyEvent::plain(Key::Up));
        let mut sink = Vec::new();
        t.drain_replies(&mut sink);
        let mut evs = Vec::new();
        t.take_events(&mut evs);
    }
    let _ = t.search(&SearchQuery::new("[a-z]+"));
    assert!(t.memory_estimate() > 0);
}

#[test]
fn memory_estimate_grows_with_scrollback() {
    let mut t = term(80, 24);
    let before = t.memory_estimate();
    feed_all(&mut t, &numbered_lines(0, 2_000));
    assert!(t.memory_estimate() > before);
}

fn select(t: &mut impl TermCore, row: u16, col: u16, kind: SelectionKind) {
    t.select(SelectionOp::Start {
        at: ViewportPoint::new(row, col),
        kind,
    });
}

fn extend(t: &mut impl TermCore, row: u16, col: u16) {
    t.select(SelectionOp::Extend {
        to: ViewportPoint::new(row, col),
    });
}

#[test]
fn selection_edges_follow_alacritty_rules() {
    let mut t = term(20, 4);
    feed_all(
        &mut t,
        b"f(a, [b]) x\r\nwrapped-word-goes-on-and-on\r\nhello",
    );

    // A char selection is empty until it is dragged.
    select(&mut t, 0, 2, SelectionKind::Char);
    assert_eq!(t.selection_text(), None);
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(frame.selection, None);
    extend(&mut t, 0, 2);
    assert_eq!(t.selection_text().as_deref(), Some("a"));
    // Dragging backwards leaves the anchor cell out.
    extend(&mut t, 0, 0);
    assert_eq!(t.selection_text().as_deref(), Some("f("));

    // A word selection on a bracket spans to its partner.
    select(&mut t, 0, 1, SelectionKind::Word);
    assert_eq!(t.selection_text().as_deref(), Some("(a, [b])"));
    select(&mut t, 0, 7, SelectionKind::Word);
    assert_eq!(t.selection_text().as_deref(), Some("[b]"));
    // Words continue across a soft wrap.
    select(&mut t, 1, 3, SelectionKind::Word);
    assert_eq!(
        t.selection_text().as_deref(),
        Some("wrapped-word-goes-on-and-on")
    );
    // Extending a word selection snaps the far end to a word boundary too.
    select(&mut t, 3, 2, SelectionKind::Word);
    extend(&mut t, 0, 10);
    assert_eq!(
        t.selection_text().as_deref(),
        Some("x\nwrapped-word-goes-on-and-on\nhello")
    );
    // A line selection covers the whole wrapped line.
    select(&mut t, 2, 0, SelectionKind::Line);
    assert_eq!(
        t.selection_text().as_deref(),
        Some("wrapped-word-goes-on-and-on\n")
    );
    t.snapshot(&mut frame);
    let span = frame.selection.expect("span");
    assert_eq!(
        (span.start, span.end, span.block),
        (ViewportPoint::new(1, 0), ViewportPoint::new(2, 19), false)
    );
    // A selection reaching the end of a row includes the newline.
    select(&mut t, 0, 10, SelectionKind::Char);
    extend(&mut t, 1, 19);
    assert_eq!(
        t.selection_text().as_deref(),
        Some("x\nwrapped-word-goes-on")
    );
}

#[test]
fn selection_text_keeps_wide_chars_and_graphemes() {
    let mut t = term(20, 3);
    feed_all(&mut t, "a中e\u{301}b".as_bytes());
    // Starting on the wide character's spacer still copies it.
    select(&mut t, 0, 2, SelectionKind::Char);
    extend(&mut t, 0, 4);
    assert_eq!(t.selection_text().as_deref(), Some("中e\u{301}b"));
    select(&mut t, 0, 3, SelectionKind::Word);
    assert_eq!(t.selection_text().as_deref(), Some("a中e\u{301}b"));
}

#[test]
fn selection_follows_its_content() {
    let mut t = term(20, 5);
    feed_all(&mut t, &numbered_lines(0, 10));
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 1), "line 7");
    select(&mut t, 1, 0, SelectionKind::Line);
    assert_eq!(t.selection_text().as_deref(), Some("line 7\n"));

    // Scrolling the viewport moves the span with the content.
    t.scroll(ViewportScroll::Up(2));
    t.snapshot(&mut frame);
    let span = frame.selection.expect("span");
    assert_eq!((span.start.row, span.end.row), (3, 3));

    // New output scrolls the selected line into history; it stays selected.
    t.scroll(ViewportScroll::Bottom);
    feed_all(&mut t, &numbered_lines(10, 20));
    assert_eq!(t.selection_text().as_deref(), Some("line 7\n"));
    t.snapshot(&mut frame);
    assert_eq!(frame.selection, None, "selection is above the viewport");
    t.scroll(ViewportScroll::Top);
    t.scroll(ViewportScroll::Down(5));
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 0), "line 5");
    let span = frame.selection.expect("span");
    assert_eq!(
        (span.start, span.end),
        (ViewportPoint::new(2, 0), ViewportPoint::new(2, 19))
    );
    // Extending while scrolled back works in viewport coordinates.
    extend(&mut t, 3, 0);
    assert_eq!(t.selection_text().as_deref(), Some("line 7\nline 8\n"));

    // A selection partly above the viewport is clipped to its top row.
    t.scroll(ViewportScroll::Down(3));
    t.snapshot(&mut frame);
    let span = frame.selection.expect("span");
    assert_eq!(
        (span.start, span.end),
        (ViewportPoint::new(0, 0), ViewportPoint::new(0, 19))
    );
}

#[test]
fn selection_is_dropped_by_erase_screen_switch_and_width_change() {
    let mut t = term(20, 5);
    let setup = |t: &mut _| {
        feed_all(t, b"\x1b[H\x1b[2Jone\r\ntwo\r\nthree");
        select(t, 1, 0, SelectionKind::Line);
        assert_eq!(t.selection_text().as_deref(), Some("two\n"));
    };

    // Erasing a line elsewhere or overprinting keeps it.
    setup(&mut t);
    feed_all(&mut t, b"\x1b[3;1H\x1b[2K\x1b[2;1HTW");
    assert_eq!(t.selection_text().as_deref(), Some("TWo\n"));
    // Erasing the selected line drops it.
    feed_all(&mut t, b"\x1b[2;2H\x1b[K");
    assert_eq!(t.selection_text(), None);

    setup(&mut t);
    feed_all(&mut t, b"\x1b[5;1H\x1b[1J");
    assert_eq!(t.selection_text(), None, "ED 1 above the cursor");

    setup(&mut t);
    feed_all(&mut t, b"\x1b[4;1H\x1b[J");
    assert_eq!(t.selection_text().as_deref(), Some("two\n"), "ED 0 below");
    feed_all(&mut t, b"\x1b[2J");
    assert_eq!(t.selection_text(), None, "ED 2");

    setup(&mut t);
    feed_all(&mut t, b"\x1b[?1049h");
    assert_eq!(t.selection_text(), None, "alternate screen");
    feed_all(&mut t, b"\x1b[?1049l");
    assert_eq!(t.selection_text(), None);

    setup(&mut t);
    feed_all(&mut t, b"\x1bc");
    assert_eq!(t.selection_text(), None, "reset");

    setup(&mut t);
    t.resize(GridSize::new(20, 8));
    assert_eq!(t.selection_text().as_deref(), Some("two\n"), "height only");
    t.resize(GridSize::new(30, 8));
    assert_eq!(t.selection_text(), None, "width change");
}

#[test]
fn search_joins_wrapped_rows_and_maps_wide_chars() {
    let mut t = term(10, 4);
    feed_all(&mut t, "abcdefghijKLM\r\n中文x e\u{301}z".as_bytes());
    let at = |line, col| termax_term::GridPoint { line, col };

    let hits = t.search(&SearchQuery::new("jK")).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!((hits[0].start, hits[0].end), (at(0, 9), at(1, 0)));

    // A wide character covers both of its cells.
    let hits = t.search(&SearchQuery::new("文x")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(2, 2), at(2, 4)));
    let hits = t.search(&SearchQuery::new("中")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(2, 0), at(2, 1)));
    let hits = t.search(&SearchQuery::new("z")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(2, 7), at(2, 7)));

    // Blank cells read as spaces up to the last column; hard line breaks end
    // a match.
    let hits = t.search(&SearchQuery::new("M {7}")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(1, 2), at(1, 9)));
    assert!(t.search(&SearchQuery::new("M {8}")).unwrap().is_empty());
    assert!(t.search(&SearchQuery::new("M.*中")).unwrap().is_empty());
    // Empty matches are skipped.
    assert!(t.search(&SearchQuery::new("q*")).unwrap().is_empty());
}

#[test]
fn osc52_load_requests_are_capped_and_keep_their_terminator() {
    let mut t = term(20, 5);
    t.select(SelectionOp::Clear);
    let mut ask = term_with_clipboard(ClipboardPolicy {
        load: ClipboardLoadMode::Ask,
        ..Default::default()
    });
    for _ in 0..17 {
        feed_all(&mut ask, b"\x1b]52;c;?\x07");
    }
    feed_all(&mut ask, b"\x1b]52;s;?\x1b\\");
    let ids: Vec<u64> = events(&mut ask)
        .into_iter()
        .filter_map(|e| match e {
            TermEvent::ClipboardLoadRequest { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 18);
    // Only the 16 most recent requests can still be answered.
    ask.answer_clipboard_load(ids[0], Some("x"));
    ask.answer_clipboard_load(ids[1], Some("x"));
    assert!(replies(&mut ask).is_empty());
    ask.answer_clipboard_load(ids[2], None);
    assert!(replies(&mut ask).is_empty(), "refusal sends nothing");
    ask.answer_clipboard_load(ids[17], Some("hi"));
    assert_eq!(replies(&mut ask), b"\x1b]52;s;aGk=\x1b\\");
}

#[test]
fn on_idle_keeps_content_selection_and_search() {
    let mut t = term(80, 24);
    feed_all(&mut t, b"needle in history\r\n");
    feed_all(&mut t, &numbered_lines(0, 3_000));
    select(&mut t, 0, 0, SelectionKind::Line);
    let selected = t.selection_text();
    let mut before = Frame::default();
    t.snapshot(&mut before);
    t.on_idle();
    t.on_idle();
    let mut after = before.clone();
    let damage = t.snapshot(&mut after);
    assert!(damage.rows.iter().all(|r| before.rows[r] == after.rows[r]));
    assert_eq!(after.rows, before.rows);
    assert_eq!(after.selection, before.selection);
    assert_eq!(t.selection_text(), selected);
    let hits = t.search(&SearchQuery::new("needle")).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].start.line < 0);
    t.scroll(ViewportScroll::Top);
    t.snapshot(&mut after);
    assert_eq!(row_text(&t, &after, 0), "needle in history");
    feed_all(&mut t, b"more\r\n");
    t.scroll(ViewportScroll::Bottom);
    t.snapshot(&mut after);
    assert_eq!(row_text(&t, &after, 22), "more");
}

#[test]
fn selection_ends_when_its_content_leaves_the_scrollback() {
    let mut t = term_without_scrollback(20, 5);
    feed_all(&mut t, b"first\r\n");
    select(&mut t, 0, 0, SelectionKind::Line);
    assert_eq!(t.selection_text().as_deref(), Some("first\n"));
    feed_all(&mut t, &numbered_lines(0, 10));
    assert_eq!(t.selection_text(), None);
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(frame.selection, None);
    // Extending a selection whose anchor is gone starts it at the first cell.
    select(&mut t, 0, 2, SelectionKind::Char);
    feed_all(&mut t, &numbered_lines(0, 10));
    extend(&mut t, 0, 3);
    assert_eq!(t.selection_text().as_deref(), Some("line"));

    // Partly scrolled off: the start is clamped to the top row, as alacritty
    // clamps it to the top of the scrolled region.
    feed_all(&mut t, b"\x1b[H\x1b[2Jaa bb\r\ncc dd\r\nee");
    select(&mut t, 0, 3, SelectionKind::Char);
    extend(&mut t, 1, 1);
    assert_eq!(t.selection_text().as_deref(), Some("bb\ncc"));
    feed_all(&mut t, b"\x1b[5;1H\n");
    assert_eq!(t.selection_text().as_deref(), Some("cc"));
    t.snapshot(&mut frame);
    let span = frame.selection.expect("span");
    assert_eq!(
        (span.start, span.end),
        (ViewportPoint::new(0, 0), ViewportPoint::new(0, 1))
    );
}

#[test]
fn alternate_screen_selection_ends_when_scrolled_off() {
    let mut t = term(20, 5);
    feed_all(&mut t, b"\x1b[?1049hfirst\r\nsecond");
    select(&mut t, 1, 0, SelectionKind::Word);
    assert_eq!(t.selection_text().as_deref(), Some("second"));
    feed_all(&mut t, b"\x1b[5;1H\n");
    assert_eq!(t.selection_text().as_deref(), Some("second"), "moved up");
    feed_all(&mut t, b"\n");
    assert_eq!(t.selection_text(), None);
}

#[test]
fn title_stack_push_and_pop() {
    let mut t = term(20, 3);
    let input = b"\x1b[22;0t\x1b]2;a\x07\x1b[22;0t\x1b]2;b\x07\x1b[23;0t\x1b[23;0t\x1b[23;0t";
    for chunk in input.chunks(4) {
        feed_all(&mut t, chunk);
    }
    assert_eq!(
        events(&mut t),
        vec![
            TermEvent::Title("a".into()),
            TermEvent::Title("b".into()),
            TermEvent::Title("a".into()),
            TermEvent::ResetTitle,
        ],
        "a pop restores the saved title; an empty stack ignores it"
    );
}

// Behaviour specific to this backend.

#[test]
fn empty_title_resets_it() {
    // alacritty reports an empty OSC 0/2 title as `Title("")` and only emits
    // `ResetTitle` when a title-stack pop restores "no title"; this backend
    // reports the empty title as a reset, which is what a UI does with it.
    let mut t = term(20, 3);
    feed_all(&mut t, b"\x1b]2;  \x07\x1b]0;x\x07\x1b]0;\x1b\\");
    assert_eq!(
        events(&mut t),
        vec![
            TermEvent::ResetTitle,
            TermEvent::Title("x".into()),
            TermEvent::ResetTitle
        ]
    );
}

#[test]
fn search_anchors_lines_and_matches_whole_graphemes() {
    // alacritty's DFA walk only honours `^` at the very top of the grid and
    // `$` rarely (see its search tests), and it ignores combining marks.
    // Here every logical line is its own haystack with complete graphemes.
    let mut t = term(10, 4);
    feed_all(&mut t, "abcdefghijKLM\r\n中文x e\u{301}z".as_bytes());
    let at = |line, col| termax_term::GridPoint { line, col };
    let hits = t.search(&SearchQuery::new("^中|^a|^K")).unwrap();
    let starts: Vec<_> = hits.iter().map(|m| m.start).collect();
    assert_eq!(
        starts,
        vec![at(0, 0), at(2, 0)],
        "wrapped rows are not line starts"
    );
    let hits = t.search(&SearchQuery::new("M +$")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(1, 2), at(1, 9)));
    let hits = t.search(&SearchQuery::new("e\u{301}z")).unwrap();
    assert_eq!((hits[0].start, hits[0].end), (at(2, 6), at(2, 7)));
}

#[test]
fn idle_compression_is_supported_and_transparent() {
    let mut t = term(80, 24);
    feed_all(&mut t, &numbered_lines(0, 5_000));
    // Linux reclaims compressed pages; Ghostty reports `Unsupported` on
    // targets without retained-mapping reclamation.
    let supported = t.compress_scrollback();
    if cfg!(target_os = "linux") {
        assert!(supported);
    }
    t.on_idle();
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(row_text(&t, &frame, 22), "line 4999");
    let hits = t.search(&SearchQuery::new(r"^line 0 *$")).unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn insert_and_delete_line_leave_the_selection_on_its_cells() {
    // alacritty moves the selection with the text when lines are inserted or
    // deleted (or a scroll region scrolls); Ghostty's tracked references stay
    // on their rows in those cases, so the selection keeps its screen cells
    // and now covers the text that moved there.
    let mut t = term(20, 5);
    feed_all(&mut t, b"one\r\ntwo\r\nthree");
    select(&mut t, 1, 0, SelectionKind::Line);
    feed_all(&mut t, b"\x1b[1;1H\x1b[M");
    assert_eq!(t.selection_text().as_deref(), Some("three\n"));
}
