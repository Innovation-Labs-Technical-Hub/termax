# termax-term

## Purpose

`termax-term` is the boundary between the SSH data path and the renderers. It defines the object-safe `TermCore` trait and its backend-neutral data types, and ships the `alacritty_terminal` backend (`AlacrittyTerm`). A per-session parser task calls `TermCore::feed` under a byte and time `Budget`, credits the SSH window with `Fed::consumed`, forwards `drain_replies` to the host, and publishes a `Frame`. A renderer calls `snapshot` at vsync and repaints only the rows in `Damage`. The crate depends on no UI toolkit. The Ghostty backend lives in `termax-term-ghostty` and implements the same trait.

## Public API entry points

- `TermCore` (see `src/lib.rs` for the full contract) and `AlacrittyTerm::new(TermConfig)`, plus `set_clipboard_policy` and `set_palette`.
- Data types in `types.rs`: `Budget`, `Fed`, `GridSize`, `Frame`, `RowView`, `Cell16`, `cell_flags`, `GRAPHEME_BIT`, `Damage`, `TermEvent`, `ClipboardPolicy`, `SelectionOp`, `SearchQuery`, `Match`, `ViewportScroll`, `CursorState`.
- `input`: pure encoders (`encode_key`, `encode_mouse`, `encode_paste`, `encode_focus`) over `InputModes`, with `PastePolicy` and `PasteOutcome`.
- `osc_scan`: `OscScanner`, `ScanAction`, `OscEvent`, `MAX_OSC_BYTES`. `Palette` and `Rgb`.

## Commands

- `cargo test -p termax-term` or `just test termax-term`.
- `just term-test` runs this crate's tests and, on Linux, the Ghostty backend's parity suite.
- `cargo clippy -p termax-term --all-targets -- -D warnings`.
- `cargo xtask perf --no-ssh --backend alacritty` measures this backend. The defaults (64 MiB per workload, 20 tabs) take a while; `--mib 1 --tabs 2` gives a quick smoke run.

## Layout

- `src/lib.rs`: the `TermCore` trait and its documented contract.
- `src/types.rs`: types exchanged across the boundary. `src/palette.rs`: colours resolved to `0xRRGGBBAA`.
- `src/alacritty.rs`: `AlacrittyTerm` and `TermConfig`. `src/input.rs`: xterm-compatible input encoders shared by every backend.
- `src/osc_scan.rs`: a streaming pre-scanner for OSC 7, OSC 133 and (optionally) OSC 52 reads, which the emulator drops.
- `tests/term_core.rs`: behavioural tests. `termax-term-ghostty/tests/term_core.rs` mirrors them for parity, so update both when behaviour changes.

## Conventions

- Implementations must never panic on arbitrary input bytes. `feed` may consume fewer bytes than offered, and the caller re-feeds the rest.
- Input encoding belongs in `input.rs` as pure functions over `InputModes`. A backend only reports its modes.
- `on_idle` may compact memory but must not change visible content, scrollback, the selection or search results.
- Keep the trait object-safe (`Box<dyn TermCore>`), `Send + 'static`, and free of backend types.

## Gotchas

- `Budget::default()` is 64 KiB or 2 ms per call, sized so a parser task never starves the UI frame. The time limit is checked between internal chunks, so it is soft.
- Defaults are hardened. `ClipboardPolicy` denies OSC 52 store and load, and caps stores at 1 MiB. `PastePolicy` strips ESC and other controls and asks for confirmation on multi-line pastes or pastes over 16 KiB.
- `OscScanner` caps OSC payloads at `MAX_OSC_BYTES` (4 MiB). Beyond that it terminates the sequence inside the parser, because vte would otherwise buffer an unterminated OSC from a hostile host without limit.
- A `Cell16::glyph` with `GRAPHEME_BIT` set is an id. Resolve it with `TermCore::grapheme`. Ids stay valid only until the next snapshot that reports full damage.

## OpenSpec

- `terminal-workspace`, module CO-04 (`FR-CO04-*`): `TermCore` boundary, parser task and window credit, frame and damage snapshots, keyboard and mouse encoding, reflow, OSC handling, hardened defaults, selection, search and viewport anchoring. Performance targets come from NFR-03 through `cargo xtask perf`. Implementation is partial; check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
