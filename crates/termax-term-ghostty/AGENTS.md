# termax-term-ghostty

## Purpose

`termax-term-ghostty` implements `termax_term::TermCore` on Ghostty's terminal core (libghostty-vt). It is the default terminal backend on Linux, used by `cargo xtask perf`. macOS and Windows use the alacritty backend until Ghostty's Zig build works there. Ghostty parses the stream and owns the grid. This crate fills in the rest of the trait so the backend behaves like `AlacrittyTerm`: OSC 7, OSC 133 and OSC 52 reads go through the shared `OscScanner`, and selection and search are ported from alacritty's rules over a cell grid reader. All FFI and the single `unsafe impl Send` live in `termax-term-ghostty-sys`, so this crate stays under the workspace `unsafe_code = "forbid"` lint.

## Public API entry points

- `GhosttyTerm::new(TermConfig)`, which panics only if Ghostty cannot allocate the terminal, and `GhosttyTerm::try_new(TermConfig) -> Result<GhosttyTerm, Error>`.
- `GhosttyTerm::compress_scrollback()`, which `TermCore::on_idle` calls.
- Everything else goes through the `TermCore` trait and the types from `termax-term`.

## Commands

Zig 0.15.2 must be on `PATH` (`just zig-install` puts it in `~/.local/bin`).

- `cargo test -p termax-term-ghostty` or `just test termax-term-ghostty`. The parity suite takes about 20 seconds.
- `just term-test` runs the alacritty and Ghostty suites together.
- `cargo clippy -p termax-term-ghostty --all-targets -- -D warnings`.
- `cargo run --release -p termax-term-ghostty --example backend_compare -- ghostty` (or `-- alacritty`) measures one backend per process.
- `just compare-backends` writes perf JSON for both backends side by side (Linux only).

## Layout

- `src/lib.rs`: `GhosttyTerm`, the feed loop, events, clipboard, cursor and title handling.
- `src/csi_scan.rs`: detects ED, EL, RIS and XTWINOPS title push and pop, whose effect the backend applies itself to match alacritty.
- `src/grid.rs`: cell-level read access to history plus the active screen. `src/selection.rs` and `src/search.rs`: alacritty-equivalent selection and regex search over `grid`.
- `tests/term_core.rs`: the parity subset of `termax-term`'s behavioural tests. Assertions stay identical wherever the backend supports the behaviour, and deliberate differences are commented in place.
- `examples/backend_compare.rs`: per-backend measurement with the same method as `cargo xtask perf`.

## Conventions

- Alacritty behaviour is the reference. When the two backends differ, port alacritty's rule here (as `selection.rs` and `search.rs` do), or document the difference in the crate docs and in the parity test.
- Do not add `unsafe` here. Extend the `Vt` API in `termax-term-ghostty-sys` instead.

## Gotchas

- The build is Linux-only for now. `just` and CI exclude both Ghostty crates on macOS and Windows, and `xtask` depends on this crate only for `target_os = "linux"`.
- The first build in a fresh target directory clones Ghostty from GitHub at the commit pinned by `libghostty-vt-sys`. Set `GHOSTTY_SOURCE_DIR` to a local checkout of that commit to build offline.
- Known differences from alacritty: `CursorStyle` events are raised at `snapshot` time, not while feeding, and an empty OSC 0/2 title is reported as `TermEvent::ResetTitle` rather than `Title("")`.
- libghostty-vt 0.2 has no search API, and its word and line selection rules differ from alacritty's. That is why `search.rs` and `selection.rs` exist. Do not replace them with Ghostty's built-ins without re-running the parity suite.
- `compress_scrollback` returns `false` where Ghostty reports compression as unsupported on the platform.

## OpenSpec

- `terminal-workspace`, module CO-04 (`FR-CO04-*`), as a second `TermCore` backend. FR-CO04-011 (tiered and compressed scrollback) is the reason for `on_idle` compaction here. Implementation is partial; check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
