//! OxTerm terminal core.
//!
//! This crate is the boundary between the SSH data path and the renderers. A
//! per-session parser task calls [`TermCore::feed`] with a byte/time
//! [`Budget`], credits the SSH window with [`Fed::consumed`], forwards
//! [`TermCore::drain_replies`] to the host, and publishes a [`Frame`]. A
//! renderer (GPU desktop, Flutter mobile, ...) refreshes its frame with
//! [`TermCore::snapshot`] at vsync and repaints only the rows in [`Damage`].
//!
//! The crate is renderer-agnostic and depends on no UI toolkit. The only
//! backend today is [`AlacrittyTerm`]; [`TermCore`] is object-safe so another
//! emulator can be swapped in behind `Box<dyn TermCore>`.

mod alacritty;
pub mod input;
mod osc_scan;
mod palette;
mod types;

pub use alacritty::{AlacrittyTerm, TermConfig};
pub use input::{
    InputModes, Key, KeyEvent, KeypadKey, Modifiers, MouseButton, MouseEncoding, MouseEvent,
    MouseEventKind, MouseTracking, PasteOutcome, PastePolicy,
};
pub use palette::{Palette, Rgb};
pub use types::*;

/// A terminal emulator as seen by the session and the renderers.
///
/// Implementations must never panic on arbitrary input bytes.
pub trait TermCore: Send + 'static {
    /// Parses up to `budget.max_bytes` of `bytes`, stopping early once
    /// `budget.max_time` has elapsed (checked between internal chunks).
    /// Returns how many leading bytes were consumed; the caller re-feeds the rest.
    fn feed(&mut self, bytes: &[u8], budget: Budget) -> Fed;

    /// Appends bytes the terminal must send back to the host (DA, DSR, colour
    /// queries, answered clipboard reads, ...).
    fn drain_replies(&mut self, out: &mut Vec<u8>);

    /// Current input-affecting modes (cursor keys, mouse tracking, paste, ...).
    fn input_modes(&self) -> InputModes;

    /// Encodes a key press for the current modes; `None` if it has no encoding.
    fn encode_key(&self, key: &KeyEvent) -> Option<Vec<u8>>;

    /// Encodes a mouse event; `None` when the application is not tracking it.
    fn encode_mouse(&self, mouse: &MouseEvent) -> Option<Vec<u8>>;

    /// Sanitises and encodes a paste, flagging when the UI should confirm.
    fn encode_paste(&self, text: &str, policy: &PastePolicy) -> PasteOutcome;

    /// Encodes a focus change; `None` unless focus reporting is on.
    fn encode_focus(&self, focused: bool) -> Option<Vec<u8>>;

    /// Resizes the grid, reflowing the primary screen and scrollback.
    fn resize(&mut self, size: GridSize);

    /// Moves the viewport through scrollback. While scrolled back the viewport
    /// stays anchored on the same content as new output arrives.
    fn scroll(&mut self, scroll: ViewportScroll);

    /// Starts, extends or clears the selection.
    fn select(&mut self, op: SelectionOp);

    /// Text of the current selection.
    fn selection_text(&self) -> Option<String>;

    /// Finds regex matches across scrollback and the screen, top to bottom.
    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Match>, SearchError>;

    /// Copies damaged rows into `frame` (reusing its allocations), updates the
    /// cursor/selection/scroll fields, increments `frame.generation`, resets the
    /// emulator's damage and returns what changed.
    fn snapshot(&mut self, frame: &mut Frame) -> Damage;

    /// Moves pending events into `out`.
    fn take_events(&mut self, out: &mut Vec<TermEvent>);

    /// Resolves a [`Cell16::glyph`] carrying [`GRAPHEME_BIT`] to its text.
    /// Ids stay valid until the next snapshot reporting full damage.
    fn grapheme(&self, glyph: u32) -> Option<&str>;

    /// Answers a [`TermEvent::ClipboardLoadRequest`]; `None` refuses it.
    /// Unknown or expired ids are ignored.
    fn answer_clipboard_load(&mut self, id: u64, text: Option<&str>);

    /// Rough number of bytes held by the grid, scrollback and side tables.
    fn memory_estimate(&self) -> usize;
}
