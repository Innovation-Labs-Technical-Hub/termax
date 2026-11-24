//! A `Send` owner for one libghostty-vt terminal.
//!
//! `oxt_term::TermCore` requires `Send`: the session moves its terminal into a
//! parser task that may run on any worker thread. The `libghostty-vt` bindings
//! mark every handle `!Send` because the C API does not promise thread safety.
//! This crate bundles one terminal with every object that points into it
//! (render state, row/cell iterators, callback state) into [`Vt`] and asserts
//! that the bundle as a whole may move between threads.
//!
//! # Why `unsafe impl Send for Vt` is sound
//!
//! * Moving is not sharing. `Vt` is not `Sync`, every mutating method takes
//!   `&mut self`, so the terminal is only ever touched by one thread at a time.
//! * No thread affinity. At the pinned Ghostty commit (see
//!   `libghostty-vt-sys` 0.2.2's build script) the only `threadlocal` in
//!   Ghostty's sources is in `src/crash/sentry.zig`, which lib-vt does not
//!   build. The default allocator is libc `malloc`, which is thread-safe.
//!   Ghostty's own search thread is not used through this API.
//! * Nothing escapes. Every `!Send` handle that refers to the terminal is a
//!   private field; the `Rc` cells shared with the registered callbacks are
//!   cloned only into those callbacks, which the terminal owns, so all clones
//!   move together. Public methods hand out borrows tied to `&self`/`&mut self`
//!   and plain-data copies only; no method returns an owned Ghostty handle or
//!   lets callers register their own callbacks.
//!
//! This is the only crate of the Ghostty backend that contains `unsafe`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use libghostty_vt as vt;

use vt::error::Result;
use vt::render::{CellIteration, CellIterator, Dirty, RowIteration, RowIterator, Snapshot};
use vt::style::{Palette, RgbColor};
use vt::terminal::{
    ClipboardLocation, CompressionMode, CompressionResult, DeviceAttributes, Mode, Options,
    ScrollViewport, Scrollbar, SizeReportSize,
};
use vt::{RenderState, Terminal};

/// Side effects collected from the emulator's callbacks during [`Vt::write`].
#[derive(Debug, Default)]
pub struct Effects {
    /// Bytes the terminal wants written back to the host (DA, DSR, ...).
    pub pty: Vec<u8>,
    /// Out-of-band notifications, in the order they happened.
    pub events: Vec<Effect>,
}

/// An out-of-band notification raised while parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// BEL.
    Bell,
    /// The title changed (OSC 0/2); carries the new title.
    Title(String),
    /// The working directory changed (OSC 7 and friends); carries Ghostty's
    /// stored value, which for OSC 7 is the URL as sent.
    Pwd(String),
    /// The host wrote to a clipboard (OSC 52 store). Reads are never reported:
    /// Ghostty ignores OSC 52 `?` requests in lib-vt.
    ClipboardWrite {
        /// Target clipboard.
        location: ClipboardLocation,
        /// Decoded `text/plain` payload (empty if not UTF-8).
        text: String,
    },
}

/// Construction options for [`Vt`].
#[derive(Clone, Copy, Debug)]
pub struct VtOptions {
    /// Columns, at least 1.
    pub cols: u16,
    /// Rows, at least 1.
    pub rows: u16,
    /// Scrollback limit **in bytes**. Ghostty's header documents this field as
    /// lines, but `Screen.init` treats it as a byte budget rounded up to whole
    /// pages; `0` disables scrollback.
    pub max_scrollback_bytes: usize,
    /// Answer for DA1/DA2/DA3 queries.
    pub device_attributes: DeviceAttributes,
}

/// One libghostty-vt terminal plus its render state, movable across threads.
pub struct Vt {
    // Field order is drop order: iterators and render state reference data the
    // terminal produced, so they are released before it.
    cells: CellIterator<'static>,
    rows: RowIterator<'static>,
    render: RenderState<'static>,
    term: Terminal<'static, 'static>,
    effects: Rc<RefCell<Effects>>,
    size_report: Rc<Cell<Option<SizeReportSize>>>,
}

// SAFETY: see the crate documentation. `Vt` owns every handle and callback
// state tied to its terminal, exposes no way to extract or share them, is not
// `Sync`, and libghostty-vt has no thread-affine state.
unsafe impl Send for Vt {}

impl std::fmt::Debug for Vt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vt").finish_non_exhaustive()
    }
}

impl Vt {
    /// Creates a terminal and registers the callbacks that feed [`Effects`].
    pub fn new(opts: VtOptions) -> Result<Self> {
        let mut term = Terminal::new(Options {
            cols: opts.cols.max(1),
            rows: opts.rows.max(1),
            max_scrollback: opts.max_scrollback_bytes,
        })?;
        let effects = Rc::new(RefCell::new(Effects::default()));
        let size_report = Rc::new(Cell::new(None));

        let fx = Rc::clone(&effects);
        term.on_pty_write(move |_, data| fx.borrow_mut().pty.extend_from_slice(data))?;
        let fx = Rc::clone(&effects);
        term.on_bell(move |_| fx.borrow_mut().events.push(Effect::Bell))?;
        let fx = Rc::clone(&effects);
        term.on_title_changed(move |t| {
            let title = t.title().unwrap_or_default().to_owned();
            fx.borrow_mut().events.push(Effect::Title(title));
        })?;
        let fx = Rc::clone(&effects);
        term.on_pwd_changed(move |t| {
            let pwd = t.pwd().unwrap_or_default().to_owned();
            fx.borrow_mut().events.push(Effect::Pwd(pwd));
        })?;
        let fx = Rc::clone(&effects);
        term.on_clipboard_write(move |_, write| {
            let text = write
                .contents()
                .find(|c| c.mime.starts_with("text/plain"))
                .map(|c| c.data.to_owned())
                .unwrap_or_default();
            fx.borrow_mut().events.push(Effect::ClipboardWrite {
                location: write.location(),
                text,
            });
            Ok(())
        })?;
        let da = opts.device_attributes;
        term.on_device_attributes(move |_| Some(da))?;
        let size = Rc::clone(&size_report);
        term.on_size(move |_| size.get())?;

        Ok(Self {
            cells: CellIterator::new()?,
            rows: RowIterator::new()?,
            render: RenderState::new()?,
            term,
            effects,
            size_report,
        })
    }

    /// Parses `bytes`; callbacks append to [`Vt::effects`].
    pub fn write(&mut self, bytes: &[u8]) {
        self.term.vt_write(bytes);
    }

    /// Pending side effects; drain them after each [`Vt::write`].
    pub fn effects(&mut self) -> std::cell::RefMut<'_, Effects> {
        self.effects.borrow_mut()
    }

    /// Resizes the grid (reflowing the primary screen). Pixel sizes of `0`
    /// mean unknown: size queries in pixels then go unanswered.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) -> Result<()> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.size_report
            .set((cell_w > 0 && cell_h > 0).then_some(SizeReportSize {
                rows,
                columns: cols,
                cell_width: u32::from(cell_w),
                cell_height: u32::from(cell_h),
            }));
        self.term
            .resize(cols, rows, u32::from(cell_w), u32::from(cell_h))
    }

    /// Moves the viewport through scrollback.
    pub fn scroll_viewport(&mut self, scroll: ScrollViewport) {
        self.term.scroll_viewport(scroll);
    }

    /// Whether a DEC/ANSI mode is set.
    pub fn mode(&self, mode: Mode) -> Result<bool> {
        self.term.mode(mode)
    }

    /// Whether the alternate screen is active.
    pub fn alt_screen(&self) -> Result<bool> {
        Ok(self.term.active_screen()? == vt::screen::Screen::Alternate)
    }

    /// Viewport position within the scrollable area.
    pub fn scrollbar(&self) -> Result<Scrollbar> {
        self.term.scrollbar()
    }

    /// Rows of history above the active area.
    pub fn scrollback_rows(&self) -> Result<usize> {
        self.term.scrollback_rows()
    }

    /// Rows of the active screen including history.
    pub fn total_rows(&self) -> Result<usize> {
        self.term.total_rows()
    }

    /// Grid width.
    pub fn cols(&self) -> Result<u16> {
        self.term.cols()
    }

    /// Grid height.
    pub fn rows(&self) -> Result<u16> {
        self.term.rows()
    }

    /// Cursor row within the active area.
    pub fn cursor_y(&self) -> Result<u16> {
        self.term.cursor_y()
    }

    /// Sets default foreground, background and the 256-colour palette.
    pub fn set_colors(&mut self, fg: RgbColor, bg: RgbColor, palette: Palette) -> Result<()> {
        self.term.set_default_fg_color(Some(fg))?;
        self.term.set_default_bg_color(Some(bg))?;
        self.term.set_default_color_palette(Some(palette))?;
        Ok(())
    }

    /// Runs one step (or a full pass) of idle scrollback compression.
    pub fn compress(&mut self, mode: CompressionMode) -> Result<CompressionResult> {
        self.term.compress(mode)
    }

    /// Refreshes the render state and visits rows.
    ///
    /// `on_state` sees the global snapshot (dirty level, cursor, colours) and
    /// returns whether every row must be visited; otherwise only rows Ghostty
    /// flagged dirty are. `on_row` receives the viewport row index. All dirty
    /// flags are cleared afterwards.
    pub fn render<C>(
        &mut self,
        ctx: &mut C,
        on_state: impl FnOnce(&mut C, &Snapshot<'static, '_>) -> Result<bool>,
        mut on_row: impl FnMut(
            &mut C,
            usize,
            &RowIteration<'static, '_>,
            &mut CellIteration<'static, '_>,
        ) -> Result<()>,
    ) -> Result<()> {
        let snapshot = self.render.update(&self.term)?;
        let all = on_state(ctx, &snapshot)?;
        let mut rows = self.rows.update(&snapshot)?;
        let mut index = 0;
        while let Some(row) = rows.next() {
            if all || row.dirty()? {
                let mut cells = self.cells.update(row)?;
                on_row(ctx, index, row, &mut cells)?;
                row.set_dirty(false)?;
            }
            index += 1;
        }
        snapshot.set_dirty(Dirty::Clean)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vt::terminal::{
        ConformanceLevel, DeviceType, PrimaryDeviceAttributes, SecondaryDeviceAttributes,
        TertiaryDeviceAttributes,
    };

    fn vt() -> Vt {
        Vt::new(VtOptions {
            cols: 10,
            rows: 3,
            max_scrollback_bytes: 0,
            device_attributes: DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(ConformanceLevel(6), &[]),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType(0),
                    firmware_version: 0,
                    rom_cartridge: 0,
                },
                tertiary: TertiaryDeviceAttributes { unit_id: 0 },
            },
        })
        .expect("terminal")
    }

    #[test]
    fn vt_is_send_and_works_after_moving_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<Vt>();

        let mut t = vt();
        t.write(b"a\x07");
        let mut t = std::thread::spawn(move || {
            t.write(b"\x1b]2;moved\x07");
            t
        })
        .join()
        .expect("thread");
        assert_eq!(
            std::mem::take(&mut t.effects().events),
            vec![Effect::Bell, Effect::Title("moved".into())]
        );
    }
}
