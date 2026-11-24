//! Streaming pre-scanner for OSC sequences the emulator does not handle itself.
//!
//! alacritty_terminal (via vte 0.15) ignores OSC 7 (working directory) and
//! OSC 133 (shell-integration prompt marks). This scanner watches the byte
//! stream before it reaches the parser, mirrors vte's OSC state transitions,
//! and survives sequences split across arbitrary chunk boundaries. Other
//! backends reuse it for the same purpose; one whose emulator drops OSC 52
//! clipboard reads can also ask for those with
//! [`OscScanner::with_clipboard_loads`].
//!
//! Every byte still reaches the emulator: the caller forwards the ranges in
//! [`ScanAction::Forward`] and handles each [`ScanAction::Event`] at the point
//! in the stream where it appears, so emulator state (such as the cursor row)
//! reflects everything before the sequence.
//!
//! It also bounds OSC length: vte buffers OSC payloads without limit when built
//! with `std`, so an unterminated OSC from a hostile host would grow memory
//! forever. Past [`MAX_OSC_BYTES`] the scanner terminates the sequence inside
//! the parser (after a byte that invalidates OSC 52 base64) and swallows the
//! rest of the oversized payload.

use std::ops::Range;

use crate::types::{ClipboardTarget, PromptMarkKind};

/// OSC payloads longer than this are cut off and discarded.
pub const MAX_OSC_BYTES: usize = 4 * 1024 * 1024;

/// Only OSC 7/133 payloads up to this size are parsed.
const MAX_CAPTURE: usize = 8 * 1024;

/// Only OSC 52 payloads up to this size are parsed; a read request is
/// `52;<selectors>;?`, so anything longer is a store.
const MAX_LOAD_CAPTURE: usize = 64;

/// Bytes injected into the parser to end an oversized OSC. `!` is not valid
/// base64, so a cut-off OSC 52 store decodes to nothing.
const OVERFLOW_TERMINATOR: &[u8] = b"!\x1b\\";

/// A recognised OSC sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OscEvent {
    /// OSC 7 working directory.
    Cwd {
        /// Host part of the `file://` URL, if any.
        host: Option<String>,
        /// Percent-decoded path.
        path: String,
    },
    /// OSC 133 prompt mark.
    Prompt(PromptMarkKind),
    /// OSC 52 clipboard read (`52;<selectors>;?`). Only reported by a scanner
    /// built with [`OscScanner::with_clipboard_loads`].
    ClipboardLoad(ClipboardLoad),
}

/// An OSC 52 clipboard read request, parsed the way vte parses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipboardLoad {
    /// Clipboard to read.
    pub target: ClipboardTarget,
    /// Selector byte echoed in the reply (`c`, `p` or `s`).
    pub selector: u8,
    /// The request ended with BEL; otherwise the reply ends with ST.
    pub bell_terminated: bool,
}

impl ClipboardLoad {
    /// The reply carrying `text`: `OSC 52 ; <selector> ; <base64> <terminator>`,
    /// byte-identical to the alacritty backend's answer.
    pub fn reply(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + text.len().div_ceil(3) * 4);
        out.extend_from_slice(b"\x1b]52;");
        out.push(self.selector);
        out.push(b';');
        base64_encode(text.as_bytes(), &mut out);
        out.extend_from_slice(if self.bell_terminated {
            b"\x07"
        } else {
            b"\x1b\\"
        });
        out
    }
}

/// Instructions for the caller, in stream order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanAction {
    /// Pass this range of the scanned chunk to the parser.
    Forward(Range<usize>),
    /// Pass these synthetic bytes to the parser.
    Inject(&'static [u8]),
    /// An OSC finished; everything before it has already been forwarded.
    Event(OscEvent),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Osc,
    /// Inside an oversized OSC whose remaining bytes are dropped.
    Swallow,
}

/// Streaming scanner state; feed every chunk of the host stream through
/// [`OscScanner::scan`] in order.
///
/// The default scanner reports OSC 7 and OSC 133.
#[derive(Debug, Default)]
pub struct OscScanner {
    state: State,
    osc_len: usize,
    capture: Vec<u8>,
    capturing: bool,
    clipboard_loads: bool,
}

impl OscScanner {
    /// Also report OSC 52 clipboard reads as [`OscEvent::ClipboardLoad`], for
    /// emulators that drop them. The sequence is still forwarded.
    pub fn with_clipboard_loads(mut self) -> Self {
        self.clipboard_loads = true;
        self
    }

    /// Scans `chunk`, appending actions to `out`. Every byte of `chunk` is
    /// either covered by exactly one `Forward` range or deliberately dropped.
    pub fn scan(&mut self, chunk: &[u8], out: &mut Vec<ScanAction>) {
        let mut fwd_start = 0;
        let mut i = 0;
        while i < chunk.len() {
            if self.state == State::Ground {
                // Fast path: plain text up to the next ESC needs no state.
                match chunk[i..].iter().position(|&b| b == 0x1b) {
                    Some(offset) => {
                        i += offset + 1;
                        self.state = State::Escape;
                        continue;
                    }
                    None => break,
                }
            }
            let byte = chunk[i];
            match self.state {
                // Taken by the fast path above.
                State::Ground => {}
                State::Escape => match byte {
                    b']' => self.begin_osc(),
                    0x18 | 0x1a | 0x20..=0x7e => self.state = State::Ground,
                    // C0 controls are executed, ESC/DEL/high bytes ignored:
                    // vte stays in the escape state for all of these.
                    _ => {}
                },
                State::Osc => match byte {
                    0x07 | 0x18 | 0x1a | 0x1b => {
                        let event = self.finish_osc(byte == 0x07);
                        self.state = if byte == 0x1b {
                            State::Escape
                        } else {
                            State::Ground
                        };
                        if let Some(event) = event {
                            push_forward(out, fwd_start..i + 1);
                            fwd_start = i + 1;
                            out.push(ScanAction::Event(event));
                        }
                    }
                    // Ignored inside OSC strings by vte.
                    0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => {}
                    _ => {
                        self.osc_len += 1;
                        if self.osc_len > MAX_OSC_BYTES {
                            push_forward(out, fwd_start..i);
                            out.push(ScanAction::Inject(OVERFLOW_TERMINATOR));
                            fwd_start = i + 1;
                            self.capturing = false;
                            self.capture.clear();
                            self.state = State::Swallow;
                        } else if self.capturing {
                            self.capture_byte(byte);
                        }
                    }
                },
                State::Swallow => match byte {
                    0x07 | 0x18 | 0x1a => {
                        fwd_start = i + 1;
                        self.state = State::Ground;
                    }
                    0x1b => {
                        // Forward the ESC: it starts whatever comes next (or forms ST).
                        fwd_start = i;
                        self.state = State::Escape;
                    }
                    _ => fwd_start = i + 1,
                },
            }
            i += 1;
        }
        push_forward(out, fwd_start..chunk.len());
    }

    fn begin_osc(&mut self) {
        self.state = State::Osc;
        self.osc_len = 0;
        self.capture.clear();
        self.capturing = true;
    }

    fn capture_byte(&mut self, byte: u8) {
        self.capture.push(byte);
        let capture = self.capture.as_slice();
        let interesting = wanted(capture, b"7;", MAX_CAPTURE)
            || wanted(capture, b"133;", MAX_CAPTURE)
            || (self.clipboard_loads && wanted(capture, b"52;", MAX_LOAD_CAPTURE));
        if !interesting {
            self.capturing = false;
            self.capture.clear();
        }
    }

    fn finish_osc(&mut self, bell_terminated: bool) -> Option<OscEvent> {
        let event = if self.capturing {
            parse_osc(&self.capture, bell_terminated)
        } else {
            None
        };
        self.capturing = false;
        self.capture.clear();
        event
    }
}

/// Whether a partial payload `capture` may still be `prefix...` within `max` bytes.
fn wanted(capture: &[u8], prefix: &[u8], max: usize) -> bool {
    if capture.len() <= prefix.len() {
        prefix.starts_with(capture)
    } else {
        capture.starts_with(prefix) && capture.len() <= max
    }
}

fn push_forward(out: &mut Vec<ScanAction>, range: Range<usize>) {
    if !range.is_empty() {
        out.push(ScanAction::Forward(range));
    }
}

fn parse_osc(payload: &[u8], bell_terminated: bool) -> Option<OscEvent> {
    if let Some(url) = payload.strip_prefix(b"7;") {
        return parse_cwd(url);
    }
    if let Some(rest) = payload.strip_prefix(b"52;") {
        return parse_clipboard_load(rest, bell_terminated);
    }
    let rest = payload.strip_prefix(b"133;")?;
    let mut parts = rest.split(|&b| b == b';');
    let kind = match parts.next()? {
        b"A" => PromptMarkKind::PromptStart,
        b"B" => PromptMarkKind::CommandStart,
        b"C" => PromptMarkKind::CommandExecuted,
        b"D" => {
            let exit_code = parts
                .next()
                .and_then(|p| std::str::from_utf8(p).ok())
                .and_then(|s| s.parse::<i32>().ok());
            PromptMarkKind::CommandFinished { exit_code }
        }
        _ => return None,
    };
    Some(OscEvent::Prompt(kind))
}

/// Mirrors vte: the first selector byte (default `c`) picks the clipboard and
/// the data parameter must be exactly `?`.
fn parse_clipboard_load(rest: &[u8], bell_terminated: bool) -> Option<OscEvent> {
    let mut parts = rest.split(|&b| b == b';');
    let selectors = parts.next()?;
    if parts.next()? != b"?" {
        return None;
    }
    let selector = selectors.first().copied().unwrap_or(b'c');
    let target = match selector {
        b'c' => ClipboardTarget::Clipboard,
        b'p' | b's' => ClipboardTarget::Selection,
        _ => return None,
    };
    Some(OscEvent::ClipboardLoad(ClipboardLoad {
        target,
        selector,
        bell_terminated,
    }))
}

/// Standard padded base64.
fn base64_encode(input: &[u8], out: &mut Vec<u8>) {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> shift) as usize & 0x3f]);
            } else {
                out.push(b'=');
            }
        }
    }
}

fn parse_cwd(url: &[u8]) -> Option<OscEvent> {
    let (host, path) = if let Some(rest) = url
        .strip_prefix(b"file://")
        .or_else(|| url.strip_prefix(b"kitty-shell-cwd://"))
    {
        let slash = rest.iter().position(|&b| b == b'/')?;
        let host = &rest[..slash];
        let host =
            (!host.is_empty()).then(|| String::from_utf8_lossy(&percent_decode(host)).into_owned());
        (host, &rest[slash..])
    } else if url.first() == Some(&b'/') {
        (None, url)
    } else {
        return None;
    };
    let path = String::from_utf8_lossy(&percent_decode(path)).into_owned();
    Some(OscEvent::Cwd { host, path })
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && let (Some(h), Some(l)) = (
                input.get(i + 1).copied().and_then(hex),
                input.get(i + 2).copied().and_then(hex),
            )
        {
            out.push(h << 4 | l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scans `parts` in order, returning forwarded bytes and events.
    fn run(parts: &[&[u8]]) -> (Vec<u8>, Vec<OscEvent>) {
        run_with(OscScanner::default(), parts)
    }

    fn run_with(mut scanner: OscScanner, parts: &[&[u8]]) -> (Vec<u8>, Vec<OscEvent>) {
        let mut forwarded = Vec::new();
        let mut events = Vec::new();
        for part in parts {
            let mut actions = Vec::new();
            scanner.scan(part, &mut actions);
            for a in actions {
                match a {
                    ScanAction::Forward(r) => forwarded.extend_from_slice(&part[r]),
                    ScanAction::Inject(b) => forwarded.extend_from_slice(b),
                    ScanAction::Event(e) => events.push(e),
                }
            }
        }
        (forwarded, events)
    }

    #[test]
    fn forwards_everything_and_detects_osc7() {
        let input: &[u8] = b"ab\x1b]7;file://box/home/t%C3%B4i\x1b\\cd";
        let (fwd, events) = run(&[input]);
        assert_eq!(fwd, input);
        assert_eq!(
            events,
            vec![OscEvent::Cwd {
                host: Some("box".into()),
                path: "/home/t\u{f4}i".into()
            }]
        );
    }

    #[test]
    fn osc133_split_byte_by_byte() {
        let input: &[u8] = b"\x1b]133;A\x07$ \x1b]133;D;127\x07";
        let parts: Vec<&[u8]> = input.chunks(1).collect();
        let (fwd, events) = run(&parts);
        assert_eq!(fwd, input);
        assert_eq!(
            events,
            vec![
                OscEvent::Prompt(PromptMarkKind::PromptStart),
                OscEvent::Prompt(PromptMarkKind::CommandFinished {
                    exit_code: Some(127)
                }),
            ]
        );
    }

    #[test]
    fn other_osc_and_escape_variants_are_ignored() {
        let (_, events) = run(&[b"\x1b]2;7;file:///x\x07\x1b(]7;/nope\x07\x1b]1337;A\x07"]);
        assert!(events.is_empty());
        // C0 controls between ESC and ']' keep vte in the escape state.
        let (_, events) = run(&[b"\x1b\n]133;B\x07"]);
        assert_eq!(events, vec![OscEvent::Prompt(PromptMarkKind::CommandStart)]);
    }

    #[test]
    fn oversized_osc_is_cut_and_swallowed() {
        let mut input = b"\x1b]52;c;".to_vec();
        input.resize(input.len() + MAX_OSC_BYTES + 10, b'A');
        input.extend_from_slice(b"\x07ok");
        let (fwd, _) = run(&[&input]);
        assert!(fwd.len() < MAX_OSC_BYTES + 16);
        assert!(fwd.ends_with(b"!\x1b\\ok"));
    }

    #[test]
    fn clipboard_loads_are_opt_in() {
        let input: &[u8] =
            b"\x1b]52;c;?\x07\x1b]52;p;?\x1b\\\x1b]52;;?\x07\x1b]52;c;aGk=\x07\x1b]52;x;?\x07";
        let (fwd, events) = run(&[input]);
        assert_eq!(fwd, input);
        assert!(events.is_empty(), "default scanner ignores OSC 52");

        let parts: Vec<&[u8]> = input.chunks(2).collect();
        let (fwd, events) = run_with(OscScanner::default().with_clipboard_loads(), &parts);
        assert_eq!(fwd, input, "the sequences still reach the emulator");
        let load = |target, selector, bell_terminated| {
            OscEvent::ClipboardLoad(ClipboardLoad {
                target,
                selector,
                bell_terminated,
            })
        };
        assert_eq!(
            events,
            vec![
                load(ClipboardTarget::Clipboard, b'c', true),
                load(ClipboardTarget::Selection, b'p', false),
                load(ClipboardTarget::Clipboard, b'c', true),
            ]
        );
    }

    #[test]
    fn clipboard_load_reply_matches_vte_format() {
        let load = ClipboardLoad {
            target: ClipboardTarget::Clipboard,
            selector: b'c',
            bell_terminated: true,
        };
        assert_eq!(load.reply("hi"), b"\x1b]52;c;aGk=\x07");
        assert_eq!(load.reply(""), b"\x1b]52;c;\x07");
        let st = ClipboardLoad {
            selector: b's',
            bell_terminated: false,
            ..load
        };
        assert_eq!(st.reply("hello!"), b"\x1b]52;s;aGVsbG8h\x1b\\");
        assert_eq!(st.reply("ab"), b"\x1b]52;s;YWI=\x1b\\");
    }

    #[test]
    fn long_clipboard_store_is_not_captured() {
        let mut input = b"\x1b]52;c;".to_vec();
        input.resize(input.len() + 200, b'A');
        input.extend_from_slice(b";?\x07");
        let (_, events) = run_with(OscScanner::default().with_clipboard_loads(), &[&input]);
        assert!(events.is_empty());
    }
}
