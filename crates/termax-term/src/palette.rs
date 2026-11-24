//! Colour palette: user-configurable defaults resolved to `0xRRGGBBAA`.

/// An opaque RGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Rgb {
    /// Construct from components.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Construct from `0xRRGGBB`.
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
        }
    }

    /// Packed `0xRRGGBBAA` with full alpha.
    pub const fn rgba(self) -> u32 {
        ((self.r as u32) << 24) | ((self.g as u32) << 16) | ((self.b as u32) << 8) | 0xff
    }

    /// Scaled to two thirds intensity, used for SGR 2 (dim) named colours.
    const fn dim(self) -> Self {
        Self {
            r: (self.r as u16 * 2 / 3) as u8,
            g: (self.g as u16 * 2 / 3) as u8,
            b: (self.b as u16 * 2 / 3) as u8,
        }
    }
}

/// Configurable default colours. Indexes 16..=255 always follow the xterm
/// 6x6x6 cube and grayscale ramp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    /// Default foreground.
    pub foreground: Rgb,
    /// Default background.
    pub background: Rgb,
    /// Cursor colour; `None` uses the foreground.
    pub cursor: Option<Rgb>,
    /// ANSI colours 0..=15.
    pub ansi: [Rgb; 16],
}

impl Default for Palette {
    /// xterm's stock colours on a black background.
    fn default() -> Self {
        const ANSI: [u32; 16] = [
            0x000000, 0xcd0000, 0x00cd00, 0xcdcd00, 0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
            0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00, 0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
        ];
        Self {
            foreground: Rgb::from_hex(0xe5e5e5),
            background: Rgb::from_hex(0x000000),
            cursor: None,
            ansi: ANSI.map(Rgb::from_hex),
        }
    }
}

/// Number of addressable colours, matching alacritty's named-colour layout:
/// 0..256 indexed, 256 fg, 257 bg, 258 cursor, 259..267 dim ANSI 0-7,
/// 267 bright fg, 268 dim fg.
pub(crate) const COLOR_COUNT: usize = 269;

/// Precomputed lookup table for a [`Palette`].
#[derive(Clone, Debug)]
pub(crate) struct ResolvedPalette {
    table: [Rgb; COLOR_COUNT],
}

impl ResolvedPalette {
    pub(crate) fn new(p: &Palette) -> Self {
        let mut table = [Rgb::default(); COLOR_COUNT];
        table[..16].copy_from_slice(&p.ansi);
        const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
        for i in 0..216 {
            table[16 + i] = Rgb::new(LEVELS[i / 36], LEVELS[(i / 6) % 6], LEVELS[i % 6]);
        }
        for i in 0..24 {
            let v = 8 + 10 * i as u8;
            table[232 + i] = Rgb::new(v, v, v);
        }
        table[256] = p.foreground;
        table[257] = p.background;
        table[258] = p.cursor.unwrap_or(p.foreground);
        for i in 0..8 {
            table[259 + i] = p.ansi[i].dim();
        }
        table[267] = p.foreground;
        table[268] = p.foreground.dim();
        Self { table }
    }

    /// Colour at `index`, clamped into range.
    pub(crate) fn get(&self, index: usize) -> Rgb {
        self.table[index.min(COLOR_COUNT - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_cube_and_ramp() {
        let p = ResolvedPalette::new(&Palette::default());
        assert_eq!(p.get(16), Rgb::new(0, 0, 0));
        assert_eq!(p.get(196), Rgb::new(255, 0, 0));
        assert_eq!(p.get(231), Rgb::new(255, 255, 255));
        assert_eq!(p.get(232), Rgb::new(8, 8, 8));
        assert_eq!(p.get(255), Rgb::new(238, 238, 238));
        assert_eq!(p.get(1).rgba(), 0xcd0000ff);
        assert_eq!(p.get(9999), p.get(268));
    }
}
