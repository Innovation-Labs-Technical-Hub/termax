//! Deterministic byte streams that stand in for real terminal traffic.

/// Small deterministic generator so runs are comparable without a `rand` dependency.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// Plain printable ASCII lines ending in CRLF, like `cat` of a log file.
pub fn ascii_lines(len: usize) -> Vec<u8> {
    let mut rng = Lcg(1);
    let mut out = Vec::with_capacity(len + 256);
    while out.len() < len {
        let width = 20 + rng.below(100);
        for _ in 0..width {
            out.push(b' ' + rng.below(95) as u8);
        }
        out.extend_from_slice(b"\r\n");
    }
    out.truncate(len);
    out
}

/// Coloured output with frequent SGR changes, like `ls --color` or a compiler log.
pub fn sgr_heavy(len: usize) -> Vec<u8> {
    let mut rng = Lcg(2);
    let mut out = Vec::with_capacity(len + 256);
    while out.len() < len {
        match rng.below(4) {
            0 => out.extend_from_slice(format!("\x1b[{}m", 30 + rng.below(8)).as_bytes()),
            1 => out.extend_from_slice(format!("\x1b[38;5;{}m", rng.below(256)).as_bytes()),
            2 => out.extend_from_slice(
                format!(
                    "\x1b[38;2;{};{};{}m",
                    rng.below(256),
                    rng.below(256),
                    rng.below(256)
                )
                .as_bytes(),
            ),
            _ => out.extend_from_slice(b"\x1b[0m"),
        }
        for _ in 0..(4 + rng.below(12)) {
            out.push(b'a' + rng.below(26) as u8);
        }
        if rng.below(8) == 0 {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.truncate(len);
    out
}

/// Vietnamese, CJK (wide) and emoji text, to exercise UTF-8 decoding and wide cells.
pub fn unicode_mix(len: usize) -> Vec<u8> {
    const WORDS: &[&str] = &[
        "Ờ",
        "Việt",
        "Nam",
        "tiếng",
        "người",
        "đường",
        "漢字",
        "終端",
        "日本語",
        "🚀",
        "✅",
        "naïve",
        "café",
    ];
    let mut rng = Lcg(3);
    let mut out = Vec::with_capacity(len + 64);
    while out.len() < len {
        out.extend_from_slice(WORDS[rng.below(WORDS.len() as u32) as usize].as_bytes());
        out.push(if rng.below(10) == 0 { b'\n' } else { b' ' });
        if out.last() == Some(&b'\n') {
            out.insert(out.len() - 1, b'\r');
        }
    }
    // Never cut a UTF-8 sequence in half at the end.
    while out.len() > len || std::str::from_utf8(&out).is_err() {
        out.pop();
    }
    out
}
