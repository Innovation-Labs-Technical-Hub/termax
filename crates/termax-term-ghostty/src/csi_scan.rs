//! Streaming detector for the few control sequences whose effect the backend
//! has to apply itself, because Ghostty either ignores them or does not
//! report them:
//!
//! * ED (`CSI Ps J`), EL (`CSI Ps K`) and RIS (`ESC c`), after which alacritty
//!   drops a selection over the erased cells (Ghostty keeps it);
//! * XTWINOPS title push and pop (`CSI 22 ; Ps t`, `CSI 23 ; Ps t`), which
//!   alacritty answers with a title event (Ghostty raises none).
//!
//! Only plain sequences count (no private marker or intermediate) with the
//! first parameter vte accepts for them, so exactly what reaches alacritty's
//! handlers is reported. Sequences may be split across chunks.

/// A sequence of interest, reported after its final byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CsiOp {
    /// ED with mode 0 (below), 1 (above), 2 (all) or 3 (scrollback).
    EraseDisplay(u16),
    /// EL with mode 0 (right), 1 (left) or 2 (all).
    EraseLine(u16),
    /// RIS, full reset.
    Reset,
    /// Save the window title on the title stack.
    PushTitle,
    /// Restore the window title from the title stack.
    PopTitle,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
}

#[derive(Debug, Default)]
pub(crate) struct CsiScanner {
    state: State,
    /// A private marker or intermediate byte disqualifies the sequence.
    plain: bool,
    /// Value of the first parameter, saturating.
    param: u16,
    /// Digits were seen for the first parameter.
    has_param: bool,
    /// The first parameter has ended.
    param_done: bool,
}

impl CsiScanner {
    /// Scans `bytes`, appending `(end, op)` for each sequence whose final byte
    /// is at `end - 1`.
    pub(crate) fn scan(&mut self, bytes: &[u8], out: &mut Vec<(usize, CsiOp)>) {
        let mut i = 0;
        while i < bytes.len() {
            if self.state == State::Ground {
                // Fast path: plain text up to the next ESC needs no state.
                match bytes[i..].iter().position(|&b| b == 0x1b) {
                    Some(offset) => {
                        i += offset + 1;
                        self.state = State::Escape;
                        continue;
                    }
                    None => break,
                }
            }
            let byte = bytes[i];
            i += 1;
            // CAN and SUB abort any sequence; ESC starts a new one.
            match byte {
                0x18 | 0x1a => {
                    self.state = State::Ground;
                    continue;
                }
                0x1b => {
                    self.state = State::Escape;
                    continue;
                }
                _ => {}
            }
            match self.state {
                // Taken by the fast path above.
                State::Ground => {}
                State::Escape => match byte {
                    b'[' => {
                        self.state = State::Csi;
                        self.plain = true;
                        self.param = 0;
                        self.has_param = false;
                        self.param_done = false;
                    }
                    b'c' => {
                        out.push((i, CsiOp::Reset));
                        self.state = State::Ground;
                    }
                    0x20..=0x2f => self.state = State::EscapeIntermediate,
                    0x30..=0x7e => self.state = State::Ground,
                    // C0 controls execute without leaving the escape.
                    _ => {}
                },
                State::EscapeIntermediate => {
                    if (0x30..=0x7e).contains(&byte) {
                        self.state = State::Ground;
                    }
                }
                State::Csi => match byte {
                    b'0'..=b'9' if !self.param_done => {
                        self.has_param = true;
                        self.param = self
                            .param
                            .saturating_mul(10)
                            .saturating_add(u16::from(byte - b'0'));
                    }
                    b'0'..=b'9' => {}
                    b';' | b':' => self.param_done = true,
                    0x20..=0x2f | 0x3c..=0x3f => self.plain = false,
                    0x40..=0x7e => {
                        if let Some(op) = self.dispatch(byte) {
                            out.push((i, op));
                        }
                        self.state = State::Ground;
                    }
                    _ => {}
                },
            }
        }
    }

    fn dispatch(&self, final_byte: u8) -> Option<CsiOp> {
        if !self.plain {
            return None;
        }
        match (final_byte, self.param) {
            (b'J', mode @ 0..=3) => Some(CsiOp::EraseDisplay(mode)),
            (b'K', mode @ 0..=2) => Some(CsiOp::EraseLine(mode)),
            (b't', 22) if self.has_param => Some(CsiOp::PushTitle),
            (b't', 23) if self.has_param => Some(CsiOp::PopTitle),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_parts(parts: &[&[u8]]) -> Vec<CsiOp> {
        let mut scanner = CsiScanner::default();
        let mut out = Vec::new();
        for part in parts {
            let mut found = Vec::new();
            scanner.scan(part, &mut found);
            for &(end, _) in &found {
                assert!(end <= part.len());
            }
            out.extend(found.into_iter().map(|(_, op)| op));
        }
        out
    }

    #[test]
    fn detects_erases_reset_and_title_stack() {
        let input: &[u8] =
            b"a\x1b[J\x1b[1J\x1b[2J\x1b[3J\x1b[K\x1b[1K\x1b[2Kz\x1bc\x1b[22;0t\x1b[23t";
        let expected = vec![
            CsiOp::EraseDisplay(0),
            CsiOp::EraseDisplay(1),
            CsiOp::EraseDisplay(2),
            CsiOp::EraseDisplay(3),
            CsiOp::EraseLine(0),
            CsiOp::EraseLine(1),
            CsiOp::EraseLine(2),
            CsiOp::Reset,
            CsiOp::PushTitle,
            CsiOp::PopTitle,
        ];
        assert_eq!(scan_parts(&[input]), expected);
        let parts: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(scan_parts(&parts), expected, "split byte by byte");
    }

    #[test]
    fn reports_offset_after_final_byte() {
        let mut scanner = CsiScanner::default();
        let mut out = Vec::new();
        scanner.scan(b"ab\x1b[2Kcd", &mut out);
        assert_eq!(out, vec![(6, CsiOp::EraseLine(2))]);
    }

    #[test]
    fn ignores_lookalikes() {
        assert!(
            scan_parts(&[
                b"[2J",            // no ESC
                b"\x1b[?2J",       // DECSED
                b"\x1b[4J\x1b[3K", // modes vte does not handle
                b"\x1b[2 J",       // intermediate
                b"\x1b(c",         // charset designation, not RIS
                b"\x1b[2\x18J",    // cancelled
                b"\x1b]2;[2J\x07", // inside an OSC
                b"\x1b[t\x1b[14t", // other window ops
            ])
            .is_empty()
        );
        // C0 controls inside the sequence are executed, not fatal.
        assert_eq!(scan_parts(&[b"\x1b[2\nJ"]), vec![CsiOp::EraseDisplay(2)]);
    }
}
