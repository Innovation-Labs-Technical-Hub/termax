//! Backend-independent encoders turning UI input into bytes for the host.
//!
//! Every function here is pure: it reads the terminal's input-related modes
//! ([`InputModes`]) and returns the byte sequence xterm would send. Backends
//! only have to report their modes; the encoding rules are shared.

use crate::types::ViewportPoint;

/// Keyboard modifier state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    /// Shift held.
    pub shift: bool,
    /// Alt / Option held.
    pub alt: bool,
    /// Control held.
    pub ctrl: bool,
    /// Meta / Super held (only affects the xterm modifier parameter).
    pub meta: bool,
}

impl Modifiers {
    /// No modifiers.
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        alt: false,
        ctrl: false,
        meta: false,
    };
    /// Shift only.
    pub const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Self::NONE
    };
    /// Alt only.
    pub const ALT: Modifiers = Modifiers {
        alt: true,
        ..Self::NONE
    };
    /// Control only.
    pub const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Self::NONE
    };

    /// xterm modifier parameter: `1 + shift + 2*alt + 4*ctrl + 8*meta`.
    pub fn xterm_param(self) -> u8 {
        1 + u8::from(self.shift)
            + 2 * u8::from(self.alt)
            + 4 * u8::from(self.ctrl)
            + 8 * u8::from(self.meta)
    }
}

/// Numeric keypad keys (affected by DECKPAM application keypad mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeypadKey {
    /// Digit `0..=9`.
    Digit(u8),
    /// `.`
    Decimal,
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `=`
    Equal,
    /// Keypad Enter.
    Enter,
}

/// A logical key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// Text character, already shifted by the keyboard layout.
    Char(char),
    /// Return.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Insert.
    Insert,
    /// Delete (forward).
    Delete,
    /// Function key `F1..=F12`.
    F(u8),
    /// Numeric keypad.
    Keypad(KeypadKey),
}

/// A key press.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    /// Key pressed.
    pub key: Key,
    /// Modifiers held.
    pub mods: Modifiers,
}

impl KeyEvent {
    /// Key press with modifiers.
    pub const fn new(key: Key, mods: Modifiers) -> Self {
        Self { key, mods }
    }

    /// Key press without modifiers.
    pub const fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Modifiers::NONE,
        }
    }
}

/// Mouse buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
}

/// Mouse event kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseEventKind {
    /// Button pressed.
    Press(MouseButton),
    /// Button released.
    Release(MouseButton),
    /// Pointer moved to a new cell, with the held button if any.
    Motion(Option<MouseButton>),
    /// Wheel up one notch.
    WheelUp,
    /// Wheel down one notch.
    WheelDown,
    /// Wheel left one notch.
    WheelLeft,
    /// Wheel right one notch.
    WheelRight,
}

/// A mouse event in viewport cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseEvent {
    /// What happened.
    pub kind: MouseEventKind,
    /// Cell under the pointer.
    pub at: ViewportPoint,
    /// Modifiers held.
    pub mods: Modifiers,
}

/// Which mouse events the application asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MouseTracking {
    /// No reporting; the UI owns the mouse.
    #[default]
    Off,
    /// Presses and releases (mode 1000).
    Click,
    /// Plus motion while a button is held (mode 1002).
    Drag,
    /// Plus all motion (mode 1003).
    Motion,
}

/// Wire format for mouse reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseEncoding {
    /// `CSI M Cb Cx Cy` with single-byte coordinates (max 223).
    #[default]
    Legacy,
    /// Mode 1005: coordinates as UTF-8 (max 2015).
    Utf8,
    /// Mode 1006: `CSI < b ; x ; y M/m`.
    Sgr,
}

/// The terminal modes that affect input encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputModes {
    /// DECCKM application cursor keys.
    pub app_cursor: bool,
    /// DECKPAM application keypad.
    pub app_keypad: bool,
    /// Mode 2004 bracketed paste.
    pub bracketed_paste: bool,
    /// Mode 1004 focus in/out reports.
    pub focus_reporting: bool,
    /// LNM: Enter sends CR LF.
    pub line_feed_new_line: bool,
    /// Mouse reporting level.
    pub mouse_tracking: MouseTracking,
    /// Mouse report format.
    pub mouse_encoding: MouseEncoding,
    /// Alternate screen with mode 1007: wheel becomes arrow keys when the
    /// application does not track the mouse.
    pub alternate_scroll: bool,
}

const ESC: u8 = 0x1b;

/// Encodes a key press, or `None` for keys with no xterm encoding.
pub fn encode_key(modes: &InputModes, ev: &KeyEvent) -> Option<Vec<u8>> {
    let mods = ev.mods;
    let param = mods.xterm_param();
    let alt_prefixed = |bytes: &[u8]| {
        let mut out = Vec::with_capacity(bytes.len() + 1);
        if mods.alt {
            out.push(ESC);
        }
        out.extend_from_slice(bytes);
        out
    };

    let out = match ev.key {
        Key::Up => cursor_key(modes, param, b'A'),
        Key::Down => cursor_key(modes, param, b'B'),
        Key::Right => cursor_key(modes, param, b'C'),
        Key::Left => cursor_key(modes, param, b'D'),
        Key::Home => cursor_key(modes, param, b'H'),
        Key::End => cursor_key(modes, param, b'F'),
        Key::Insert => tilde_key(2, param),
        Key::Delete => tilde_key(3, param),
        Key::PageUp => tilde_key(5, param),
        Key::PageDown => tilde_key(6, param),
        Key::F(n @ 1..=4) => {
            let letter = b"PQRS"[usize::from(n - 1)];
            if param > 1 {
                format!("\x1b[1;{param}{}", letter as char).into_bytes()
            } else {
                vec![ESC, b'O', letter]
            }
        }
        Key::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde_key(CODES[usize::from(n - 5)], param)
        }
        Key::F(_) => return None,
        Key::Enter => alt_prefixed(if modes.line_feed_new_line {
            b"\r\n"
        } else {
            b"\r"
        }),
        Key::Tab if mods.shift => alt_prefixed(b"\x1b[Z"),
        Key::Tab => alt_prefixed(b"\t"),
        Key::Backspace => alt_prefixed(if mods.ctrl { &[0x08] } else { &[0x7f] }),
        Key::Escape => alt_prefixed(&[ESC]),
        Key::Char(c) => {
            let ctrl = if mods.ctrl { ctrl_byte(c) } else { None };
            match ctrl {
                Some(b) => alt_prefixed(&[b]),
                None => {
                    let mut buf = [0u8; 4];
                    alt_prefixed(c.encode_utf8(&mut buf).as_bytes())
                }
            }
        }
        Key::Keypad(k) => keypad_key(modes, k),
    };
    Some(out)
}

fn cursor_key(modes: &InputModes, param: u8, letter: u8) -> Vec<u8> {
    if param > 1 {
        format!("\x1b[1;{param}{}", letter as char).into_bytes()
    } else if modes.app_cursor {
        vec![ESC, b'O', letter]
    } else {
        vec![ESC, b'[', letter]
    }
}

fn tilde_key(code: u8, param: u8) -> Vec<u8> {
    if param > 1 {
        format!("\x1b[{code};{param}~").into_bytes()
    } else {
        format!("\x1b[{code}~").into_bytes()
    }
}

fn keypad_key(modes: &InputModes, key: KeypadKey) -> Vec<u8> {
    let (app, normal): (u8, &[u8]) = match key {
        KeypadKey::Digit(d) => {
            let d = d.min(9);
            (b'p' + d, &b"0123456789"[usize::from(d)..usize::from(d) + 1])
        }
        KeypadKey::Decimal => (b'n', b"."),
        KeypadKey::Add => (b'k', b"+"),
        KeypadKey::Subtract => (b'm', b"-"),
        KeypadKey::Multiply => (b'j', b"*"),
        KeypadKey::Divide => (b'o', b"/"),
        KeypadKey::Equal => (b'X', b"="),
        KeypadKey::Enter => (
            b'M',
            if modes.line_feed_new_line {
                b"\r\n"
            } else {
                b"\r"
            },
        ),
    };
    if modes.app_keypad {
        vec![ESC, b'O', app]
    } else {
        normal.to_vec()
    }
}

/// Control-key mapping following xterm (`Ctrl+A` = 0x01, `Ctrl+[` = ESC, ...).
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c {
        'a'..='z' | 'A'..='Z' => (c as u8) & 0x1f,
        '@' | ' ' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '~' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

/// Encodes a mouse event according to the active tracking mode and format.
///
/// Returns `None` when the application does not want this event (the UI should
/// then handle it locally, e.g. as a selection) or when coordinates cannot be
/// represented in the active format.
pub fn encode_mouse(modes: &InputModes, ev: &MouseEvent) -> Option<Vec<u8>> {
    if modes.mouse_tracking == MouseTracking::Off {
        if !modes.alternate_scroll {
            return None;
        }
        let key = match ev.kind {
            MouseEventKind::WheelUp => Key::Up,
            MouseEventKind::WheelDown => Key::Down,
            _ => return None,
        };
        return encode_key(modes, &KeyEvent::plain(key));
    }

    let button_code = |b: MouseButton| match b {
        MouseButton::Left => 0u32,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let sgr = modes.mouse_encoding == MouseEncoding::Sgr;
    let mut release = false;
    let mut code = match ev.kind {
        MouseEventKind::Press(b) => button_code(b),
        MouseEventKind::Release(b) => {
            release = true;
            if sgr { button_code(b) } else { 3 }
        }
        MouseEventKind::Motion(held) => match held {
            Some(b) if modes.mouse_tracking >= MouseTracking::Drag => button_code(b) + 32,
            None if modes.mouse_tracking == MouseTracking::Motion => 3 + 32,
            _ => return None,
        },
        MouseEventKind::WheelUp => 64,
        MouseEventKind::WheelDown => 65,
        MouseEventKind::WheelLeft => 66,
        MouseEventKind::WheelRight => 67,
    };
    if ev.mods.shift {
        code += 4;
    }
    if ev.mods.alt || ev.mods.meta {
        code += 8;
    }
    if ev.mods.ctrl {
        code += 16;
    }

    let x = u32::from(ev.at.col) + 1;
    let y = u32::from(ev.at.row) + 1;
    match modes.mouse_encoding {
        MouseEncoding::Sgr => {
            let fin = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{fin}").into_bytes())
        }
        MouseEncoding::Legacy => {
            let enc = |v: u32| u8::try_from(32 + v).ok();
            Some(vec![ESC, b'[', b'M', enc(code)?, enc(x)?, enc(y)?])
        }
        MouseEncoding::Utf8 => {
            let mut out = vec![ESC, b'[', b'M'];
            for v in [code, x, y] {
                let v = 32 + v;
                if v > 2047 {
                    return None;
                }
                let mut buf = [0u8; 4];
                out.extend_from_slice(char::from_u32(v)?.encode_utf8(&mut buf).as_bytes());
            }
            Some(out)
        }
    }
}

/// How pasted text is sanitised and when the UI should confirm first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PastePolicy {
    /// Keep ESC characters (otherwise stripped like other controls).
    pub allow_escape: bool,
    /// Ask for confirmation when the text contains a line break.
    pub confirm_multiline: bool,
    /// Ask for confirmation when the encoded text exceeds this many bytes.
    pub confirm_over_bytes: usize,
}

impl Default for PastePolicy {
    fn default() -> Self {
        Self {
            allow_escape: false,
            confirm_multiline: true,
            confirm_over_bytes: 16 * 1024,
        }
    }
}

/// Result of [`encode_paste`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PasteOutcome {
    /// Bytes to send, already wrapped for bracketed paste when active.
    pub bytes: Vec<u8>,
    /// The UI should ask the user before sending `bytes`.
    pub needs_confirmation: bool,
    /// Number of lines in the pasted text (line breaks + 1).
    pub lines: usize,
    /// Number of characters removed by sanitisation.
    pub removed: usize,
}

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// Sanitises and encodes pasted text.
///
/// Control characters (C0, DEL, C1) are removed except TAB, CR and LF; ESC is
/// kept only with [`PastePolicy::allow_escape`]. Line breaks (`\r\n`, `\n`,
/// `\r`) become `\r`. In bracketed-paste mode the text is wrapped in
/// `ESC[200~ … ESC[201~` and any embedded bracket markers are removed so the
/// pasted content cannot terminate the bracket early.
pub fn encode_paste(modes: &InputModes, text: &str, policy: &PastePolicy) -> PasteOutcome {
    let mut out = String::with_capacity(text.len());
    let mut removed = 0;
    let mut breaks = 0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                breaks += 1;
                out.push('\r');
            }
            '\n' => {
                breaks += 1;
                out.push('\r');
            }
            '\t' => out.push('\t'),
            '\x1b' if policy.allow_escape => out.push(c),
            c if c.is_control() => removed += 1,
            c => out.push(c),
        }
    }

    if modes.bracketed_paste {
        // Removing one marker can splice a new one together, so repeat until stable.
        loop {
            let before = out.len();
            out = out.replace(PASTE_END, "").replace(PASTE_START, "");
            if out.len() == before {
                break;
            }
            removed += 1;
        }
    }

    let mut bytes = Vec::with_capacity(out.len() + 12);
    if modes.bracketed_paste {
        bytes.extend_from_slice(PASTE_START.as_bytes());
        bytes.extend_from_slice(out.as_bytes());
        bytes.extend_from_slice(PASTE_END.as_bytes());
    } else {
        bytes.extend_from_slice(out.as_bytes());
    }

    let needs_confirmation =
        (policy.confirm_multiline && breaks > 0) || out.len() > policy.confirm_over_bytes;
    PasteOutcome {
        bytes,
        needs_confirmation,
        lines: breaks + 1,
        removed,
    }
}

/// Encodes a focus change; `None` unless mode 1004 is enabled.
pub fn encode_focus(modes: &InputModes, focused: bool) -> Option<Vec<u8>> {
    modes.focus_reporting.then(|| {
        if focused {
            b"\x1b[I".to_vec()
        } else {
            b"\x1b[O".to_vec()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(modes: &InputModes, key: Key, mods: Modifiers) -> Vec<u8> {
        encode_key(modes, &KeyEvent::new(key, mods)).expect("encodable")
    }

    #[test]
    fn arrows_follow_decckm() {
        let normal = InputModes::default();
        let app = InputModes {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(key(&normal, Key::Up, Modifiers::NONE), b"\x1b[A");
        assert_eq!(key(&app, Key::Up, Modifiers::NONE), b"\x1bOA");
        assert_eq!(key(&normal, Key::Left, Modifiers::NONE), b"\x1b[D");
        assert_eq!(key(&app, Key::Home, Modifiers::NONE), b"\x1bOH");
        assert_eq!(key(&normal, Key::End, Modifiers::NONE), b"\x1b[F");
    }

    /// Verifies: FR-CO04-005 acceptance 1
    #[test]
    fn modifiers_use_csi_1_m() {
        let m = InputModes {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(key(&m, Key::Up, Modifiers::SHIFT), b"\x1b[1;2A");
        assert_eq!(key(&m, Key::Right, Modifiers::ALT), b"\x1b[1;3C");
        assert_eq!(key(&m, Key::Left, Modifiers::CTRL), b"\x1b[1;5D");
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(key(&m, Key::Down, ctrl_shift), b"\x1b[1;6B");
        assert_eq!(key(&m, Key::PageUp, Modifiers::CTRL), b"\x1b[5;5~");
        assert_eq!(key(&m, Key::F(1), Modifiers::SHIFT), b"\x1b[1;2P");
        assert_eq!(key(&m, Key::F(5), Modifiers::CTRL), b"\x1b[15;5~");
    }

    #[test]
    fn function_and_navigation_keys() {
        let m = InputModes::default();
        assert_eq!(key(&m, Key::F(1), Modifiers::NONE), b"\x1bOP");
        assert_eq!(key(&m, Key::F(4), Modifiers::NONE), b"\x1bOS");
        assert_eq!(key(&m, Key::F(5), Modifiers::NONE), b"\x1b[15~");
        assert_eq!(key(&m, Key::F(12), Modifiers::NONE), b"\x1b[24~");
        assert_eq!(key(&m, Key::PageUp, Modifiers::NONE), b"\x1b[5~");
        assert_eq!(key(&m, Key::PageDown, Modifiers::NONE), b"\x1b[6~");
        assert_eq!(key(&m, Key::Delete, Modifiers::NONE), b"\x1b[3~");
        assert!(encode_key(&m, &KeyEvent::plain(Key::F(13))).is_none());
    }

    #[test]
    fn control_alt_and_simple_keys() {
        let m = InputModes::default();
        assert_eq!(key(&m, Key::Char('c'), Modifiers::CTRL), [0x03]);
        assert_eq!(key(&m, Key::Char('A'), Modifiers::CTRL), [0x01]);
        assert_eq!(key(&m, Key::Char('['), Modifiers::CTRL), [0x1b]);
        assert_eq!(key(&m, Key::Char(' '), Modifiers::CTRL), [0x00]);
        assert_eq!(key(&m, Key::Char('x'), Modifiers::ALT), b"\x1bx");
        let ctrl_alt = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(key(&m, Key::Char('a'), ctrl_alt), b"\x1b\x01");
        assert_eq!(key(&m, Key::Char('ờ'), Modifiers::NONE), "ờ".as_bytes());
        assert_eq!(key(&m, Key::Backspace, Modifiers::NONE), [0x7f]);
        assert_eq!(key(&m, Key::Backspace, Modifiers::CTRL), [0x08]);
        assert_eq!(key(&m, Key::Enter, Modifiers::NONE), b"\r");
        assert_eq!(key(&m, Key::Tab, Modifiers::SHIFT), b"\x1b[Z");
        let lnm = InputModes {
            line_feed_new_line: true,
            ..Default::default()
        };
        assert_eq!(key(&lnm, Key::Enter, Modifiers::NONE), b"\r\n");
    }

    #[test]
    fn keypad_follows_deckpam() {
        let app = InputModes {
            app_keypad: true,
            ..Default::default()
        };
        assert_eq!(
            key(&app, Key::Keypad(KeypadKey::Digit(5)), Modifiers::NONE),
            b"\x1bOu"
        );
        assert_eq!(
            key(&app, Key::Keypad(KeypadKey::Enter), Modifiers::NONE),
            b"\x1bOM"
        );
        let normal = InputModes::default();
        assert_eq!(
            key(&normal, Key::Keypad(KeypadKey::Digit(5)), Modifiers::NONE),
            b"5"
        );
    }

    fn mouse(kind: MouseEventKind, row: u16, col: u16) -> MouseEvent {
        MouseEvent {
            kind,
            at: ViewportPoint::new(row, col),
            mods: Modifiers::NONE,
        }
    }

    #[test]
    fn mouse_off_reports_nothing() {
        let m = InputModes::default();
        assert!(encode_mouse(&m, &mouse(MouseEventKind::Press(MouseButton::Left), 0, 0)).is_none());
    }

    #[test]
    fn mouse_sgr_and_legacy() {
        let sgr = InputModes {
            mouse_tracking: MouseTracking::Click,
            mouse_encoding: MouseEncoding::Sgr,
            ..Default::default()
        };
        let press = mouse(MouseEventKind::Press(MouseButton::Left), 4, 9);
        assert_eq!(encode_mouse(&sgr, &press).unwrap(), b"\x1b[<0;10;5M");
        let release = mouse(MouseEventKind::Release(MouseButton::Right), 4, 9);
        assert_eq!(encode_mouse(&sgr, &release).unwrap(), b"\x1b[<2;10;5m");
        let wheel = mouse(MouseEventKind::WheelDown, 0, 0);
        assert_eq!(encode_mouse(&sgr, &wheel).unwrap(), b"\x1b[<65;1;1M");
        let mut ctrl_press = press;
        ctrl_press.mods = Modifiers::CTRL;
        assert_eq!(encode_mouse(&sgr, &ctrl_press).unwrap(), b"\x1b[<16;10;5M");
        // Click tracking ignores motion.
        assert!(
            encode_mouse(
                &sgr,
                &mouse(MouseEventKind::Motion(Some(MouseButton::Left)), 1, 1)
            )
            .is_none()
        );

        let legacy = InputModes {
            mouse_tracking: MouseTracking::Click,
            ..Default::default()
        };
        assert_eq!(
            encode_mouse(&legacy, &press).unwrap(),
            [0x1b, b'[', b'M', 32, 32 + 10, 32 + 5]
        );
        assert_eq!(
            encode_mouse(&legacy, &release).unwrap(),
            [0x1b, b'[', b'M', 35, 32 + 10, 32 + 5]
        );
        // Legacy cannot encode column 300.
        assert!(
            encode_mouse(
                &legacy,
                &mouse(MouseEventKind::Press(MouseButton::Left), 0, 300)
            )
            .is_none()
        );
        let utf8 = InputModes {
            mouse_encoding: MouseEncoding::Utf8,
            ..legacy
        };
        let far = encode_mouse(
            &utf8,
            &mouse(MouseEventKind::Press(MouseButton::Left), 0, 300),
        )
        .unwrap();
        assert_eq!(&far[..4], &[0x1b, b'[', b'M', 32]);
        assert_eq!(std::str::from_utf8(&far[4..]).unwrap(), "\u{14d}!");
    }

    #[test]
    fn mouse_motion_levels() {
        let drag = InputModes {
            mouse_tracking: MouseTracking::Drag,
            mouse_encoding: MouseEncoding::Sgr,
            ..Default::default()
        };
        let held = mouse(MouseEventKind::Motion(Some(MouseButton::Left)), 0, 0);
        let free = mouse(MouseEventKind::Motion(None), 0, 0);
        assert_eq!(encode_mouse(&drag, &held).unwrap(), b"\x1b[<32;1;1M");
        assert!(encode_mouse(&drag, &free).is_none());
        let any = InputModes {
            mouse_tracking: MouseTracking::Motion,
            ..drag
        };
        assert_eq!(encode_mouse(&any, &free).unwrap(), b"\x1b[<35;1;1M");
    }

    #[test]
    fn alternate_scroll_turns_wheel_into_arrows() {
        let m = InputModes {
            alternate_scroll: true,
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(
            encode_mouse(&m, &mouse(MouseEventKind::WheelUp, 0, 0)).unwrap(),
            b"\x1bOA"
        );
    }

    #[test]
    fn paste_bracketed_strips_injection_and_controls() {
        let m = InputModes {
            bracketed_paste: true,
            ..Default::default()
        };
        let policy = PastePolicy {
            allow_escape: true,
            ..Default::default()
        };
        let out = encode_paste(&m, "echo hi\x1b[201~; rm -rf ~\x07", &policy);
        assert_eq!(out.bytes, b"\x1b[200~echo hi; rm -rf ~\x1b[201~");
        assert!(out.removed >= 2);
        // Nested markers that re-form after one removal are also removed.
        let out = encode_paste(&m, "a\x1b[20\x1b[201~1~b", &policy);
        assert_eq!(out.bytes, b"\x1b[200~ab\x1b[201~");
    }

    #[test]
    fn paste_without_escape_permission_drops_esc() {
        let m = InputModes::default();
        let out = encode_paste(&m, "a\x1b[31mb\x00c\u{9b}d\te", &PastePolicy::default());
        assert_eq!(out.bytes, b"a[31mbcd\te");
        assert_eq!(out.removed, 3);
        assert!(!out.needs_confirmation);
    }

    #[test]
    fn paste_newlines_become_cr_and_need_confirmation() {
        let m = InputModes::default();
        let out = encode_paste(&m, "ls\r\ncd /\nexit", &PastePolicy::default());
        assert_eq!(out.bytes, b"ls\rcd /\rexit");
        assert_eq!(out.lines, 3);
        assert!(out.needs_confirmation);

        let big = "x".repeat(100);
        let small_limit = PastePolicy {
            confirm_over_bytes: 10,
            ..Default::default()
        };
        assert!(encode_paste(&m, &big, &small_limit).needs_confirmation);
        let lenient = PastePolicy {
            confirm_multiline: false,
            ..Default::default()
        };
        assert!(!encode_paste(&m, "a\nb", &lenient).needs_confirmation);
    }

    #[test]
    fn focus_only_when_enabled() {
        assert!(encode_focus(&InputModes::default(), true).is_none());
        let m = InputModes {
            focus_reporting: true,
            ..Default::default()
        };
        assert_eq!(encode_focus(&m, true).unwrap(), b"\x1b[I");
        assert_eq!(encode_focus(&m, false).unwrap(), b"\x1b[O");
    }
}
