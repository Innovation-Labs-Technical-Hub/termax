# termax-term-ghostty-sys

## Purpose

`termax-term-ghostty-sys` owns one libghostty-vt terminal together with every object that points into it (render state, row and cell iterators, callback state), bundled in `Vt`. It asserts that the bundle may move between threads. `termax_term::TermCore` requires `Send`, because a session moves its terminal into a parser task that can run on any worker thread, while the `libghostty-vt` bindings mark every handle `!Send`. This crate holds the only `unsafe` code of the Ghostty backend: a single `unsafe impl Send for Vt`, whose soundness argument is written out in the crate docs at the top of `src/lib.rs`. Read that argument before changing anything here.

## Public API entry points

- `Vt::new(VtOptions)`, `write`, `effects()` (`Effects` and `Effect` collected from Ghostty callbacks), `resize`, `scroll_viewport`, `mode`, `alt_screen`, `scrollbar`, `scrollback_rows`, `total_rows`, `cols`, `rows`, `cursor_y`, `set_colors`, `compress`, `convert`, `read_row` (into `RowText` and `CellSpan`), `render`.
- Tracked grid references: `set_mark`, `mark`, `clear_marks`, with `MARKS` slots.
- `vt`, a re-export of `libghostty_vt` for its plain-data types.

## Commands

Zig 0.15.2 must be on `PATH`.

- `cargo test -p termax-term-ghostty-sys` (one unit test). The real coverage comes from `cargo test -p termax-term-ghostty`.
- `cargo clippy -p termax-term-ghostty-sys --all-targets -- -D warnings`.

## Layout

- `src/lib.rs`: the whole crate, including the `Send` soundness argument.
- `Cargo.toml`: the exact `=0.2.2` pins and the lint override (`unsafe_code = "allow"`, with clippy settings mirroring the workspace).

## Conventions

- Keep `unsafe` to the single `unsafe impl Send for Vt`. Anything else that seems to need `unsafe` belongs in the upstream bindings, not here.
- `Vt` must stay `!Sync`, and every mutating method takes `&mut self`.
- Every `!Send` handle that refers to the terminal must remain a private field. Public methods return borrows tied to `&self` or `&mut self`, or plain-data copies. They never return an owned Ghostty handle or accept caller-registered callbacks.

## Gotchas

- `libghostty-vt` and `libghostty-vt-sys` are pinned exactly (`=0.2.2`), because the sys crate's build script pins a Ghostty commit per release, so a semver-compatible bump silently changes the emulator. A version bump must re-check the soundness argument, in particular that lib-vt still builds no `threadlocal` and still uses a thread-safe allocator, and must re-run the Ghostty parity suite.
- The `VtOptions` scrollback limit is in bytes, not lines. Ghostty's header says lines, but `Screen.init` treats the value as a byte budget rounded up to whole pages, and `0` disables scrollback.
- Ghostty never reports OSC 52 clipboard reads through `Effects`. `termax-term-ghostty` detects them with `OscScanner::with_clipboard_loads` instead.
- The build needs Zig 0.15.2 and is Linux-only for now. The first build clones Ghostty unless `GHOSTTY_SOURCE_DIR` points at a local checkout of the pinned commit.

## OpenSpec

- No capability of its own. It supports `terminal-workspace` (CO-04) through `termax-term-ghostty`.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
