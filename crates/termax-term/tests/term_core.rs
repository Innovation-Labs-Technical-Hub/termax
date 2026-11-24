//! Behavioural tests for the alacritty-backed `TermCore`.

use std::time::Duration;

use termax_term::{
    AlacrittyTerm, Budget, ClipboardLoadMode, ClipboardPolicy, ClipboardStoreMode, ClipboardTarget,
    Frame, GridSize, Key, KeyEvent, Modifiers, MouseButton, MouseEvent, MouseEventKind,
    PastePolicy, PromptMarkKind, SearchQuery, SelectionKind, SelectionOp, TermConfig, TermCore,
    TermEvent, UnderlineStyle, ViewportPoint, ViewportScroll, cell_flags,
};

fn term(cols: u16, rows: u16) -> AlacrittyTerm {
    AlacrittyTerm::new(TermConfig {
        size: GridSize::new(cols, rows),
        ..Default::default()
    })
}

fn feed_all(t: &mut AlacrittyTerm, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let fed = t.feed(rest, Budget::UNLIMITED);
        assert!(fed.consumed > 0, "feed must make progress");
        rest = &rest[fed.consumed..];
    }
}

fn events(t: &mut AlacrittyTerm) -> Vec<TermEvent> {
    let mut out = Vec::new();
    t.take_events(&mut out);
    out
}

fn replies(t: &mut AlacrittyTerm) -> Vec<u8> {
    let mut out = Vec::new();
    t.drain_replies(&mut out);
    out
}

fn row_text(t: &AlacrittyTerm, frame: &Frame, row: usize) -> String {
    frame
        .row_text(row, |g| t.grapheme(g).map(str::to_owned))
        .trim_end()
        .to_owned()
}

#[test]
fn trait_is_object_safe() {
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
fn clipboard_policy_gates_osc52() {
    // "hello" in base64.
    let store = b"\x1b]52;c;aGVsbG8=\x07";
    let load = b"\x1b]52;c;?\x07";

    let mut deny = term(20, 5);
    feed_all(&mut deny, store);
    feed_all(&mut deny, load);
    assert!(events(&mut deny).is_empty());
    assert!(replies(&mut deny).is_empty());

    let mut allow = AlacrittyTerm::new(TermConfig {
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

    let mut ask = AlacrittyTerm::new(TermConfig {
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

    let mut small = AlacrittyTerm::new(TermConfig {
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

    let idle = t.snapshot(&mut frame);
    assert_eq!(
        idle.rows.iter().collect::<Vec<_>>(),
        vec![2],
        "only the cursor row"
    );
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
fn cell16_is_sixteen_bytes() {
    assert_eq!(std::mem::size_of::<termax_term::Cell16>(), 16);
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

fn numbered_lines(from: usize, to: usize) -> Vec<u8> {
    (from..to)
        .map(|i| format!("line {i}\r\n"))
        .collect::<String>()
        .into_bytes()
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
fn scrollback_is_capped() {
    let mut t = AlacrittyTerm::new(TermConfig {
        size: GridSize::new(20, 5),
        scrollback_lines: 10,
        ..Default::default()
    });
    feed_all(&mut t, &numbered_lines(0, 100));
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert_eq!(frame.history_len, 10);
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

#[test]
fn cursor_shape_events_and_state() {
    let mut t = term(20, 3);
    feed_all(&mut t, b"\x1b[5 q");
    assert!(events(&mut t).contains(&TermEvent::CursorStyle {
        shape: termax_term::CursorShape::Beam,
        blinking: true
    }));
    feed_all(&mut t, b"\x1b[?25l");
    let mut frame = Frame::default();
    t.snapshot(&mut frame);
    assert!(!frame.cursor.visible);
    assert_eq!(frame.cursor.shape, termax_term::CursorShape::Beam);
}

#[test]
fn color_query_is_answered_from_palette() {
    let mut t = term(20, 3);
    feed_all(&mut t, b"\x1b]11;?\x07");
    assert_eq!(replies(&mut t), b"\x1b]11;rgb:0000/0000/0000\x07");
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
        // Some random sequences resize the grid; keep encoders exercised too.
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
