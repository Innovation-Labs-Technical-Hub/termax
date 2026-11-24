# Ghostty (libghostty-vt) backend spike: comparison with alacritty

Date: 2026-11-24. Branch `feat/p0-foundations`. Machine: i9-12900K, Linux x86_64, rustc 1.99.1.

## Outcome

`GhosttyTerm` implements `oxt_term::TermCore` on libghostty-vt and passes 28 parity tests ported from
`crates/oxt-term/tests/term_core.rs`. Ghostty meets both targets the alacritty backend misses.
Parser throughput is about 4.6 times higher, and memory per tab is 2.8 times lower before compression
and 35 times lower after it. The costs are a Zig toolchain, network access at build time, and a
third-party, pre-1.0 binding.

| Metric (release, same method as `xtask perf`) | alacritty | ghostty | ghostty + idle compression | Target |
|---|---|---|---|---|
| Parser, ASCII, 64 MiB, default `Budget`, 120×40, best of 3 | 144 MB/s | **658–666 MB/s** | n/a | ≥ 300 |
| Memory per tab (RSS), 80×24 + 10k lines, 20 tabs | 18.91 MiB | **6.64 MiB** | **0.53 MiB** | ≤ 3 |
| Memory per tab (core estimate) | 18.59 MiB | 6.25 MiB | 6.25 MiB (estimate ignores compression) | n/a |
| Snapshot, full 200×60 (median) | 0.050 ms | 0.190 ms | n/a | ≤ 1.5 |
| Snapshot, one line 200×60 (median) | 0.002 ms | 0.007 ms | n/a | ≤ 0.2 |

Each figure comes from two runs; the runs differed by less than 1.5%. The alacritty run here measured
144 MB/s against the baseline's 158 MB/s, which is ordinary machine variance. Reproduce with:

```sh
cargo run --release -p oxt-term-ghostty --example backend_compare -- alacritty
cargo run --release -p oxt-term-ghostty --example backend_compare -- ghostty
cargo run --release -p oxt-term-ghostty --example backend_compare -- ghostty-compressed
```

The example copies `parser_throughput`, `memory_per_tab` and `snapshot_costs` from `crates/xtask/src/perf.rs`,
including the `ascii_lines` LCG workload byte for byte. It measures one backend per process, so memory freed
by one backend cannot hide the growth of the other.

About the numbers:
- Ghostty is built with `-Dcpu=baseline`, the binding's portable default. Its SIMD code (Highway and simdutf)
  dispatches at runtime, so the throughput figure does not depend on compiling for the host CPU.
- Without compression the result is 6.6 MiB per tab. That is close to the cost of 10k × 80 cells at 8 bytes
  each (6.1 MiB): packed cells beat alacritty's 24-byte cells but do not reach ≤ 3 MiB on their own.
- The ≤ 3 MiB target is met only with Ghostty's idle scrollback compression (`Terminal::compress`, exposed as
  `GhosttyTerm::compress_scrollback`). The test history is `"%8d " + 71×'x'`, which compresses extremely well.
  Real logs will compress less, so treat 0.53 MiB as a best case and check it against real traffic.
  Compressed pages are restored transparently when they are read, for example on scroll.
- A full snapshot costs about 4 times more than with alacritty because every cell goes through several C getter
  calls. It is still well inside the budget.

## Binding chosen: `libghostty-vt` 0.2.2 + `libghostty-vt-sys` 0.2.2

| | `libghostty-vt` 0.2.2 | `ghostty-vt` 0.9.0+ghostty-vt.1.0.1 |
|---|---|---|
| Licence | MIT OR Apache-2.0 (both crates) | Apache-2.0 (both crates) |
| MSRV | **1.90** | 1.96 |
| Origin | Upstream binding (Uzaaft/libghostty-rs) | Fork of the binding on the left (`prabirshrestha/gpui-cn`) |
| Grid, cells and dirty tracking | `RenderState` with global and per-row dirty flags and a cell iterator | The same API (forked) |
| Extras | none | search, snapshots, queued events instead of callbacks |
| Ghostty source | git clone of pinned commit `a887df42…` (1.3.2-dev) | Release tarball `vt-1.0.1`, SHA-256 pinned, plus a deps tarball |
| Build dependencies | none by default | ureq + rustls, zip, tar, flate2, sha2 |
| Runtime dependencies | bitflags, int-enum | the same plus `dirs` (writes terminfo to a cache directory) |

I picked `libghostty-vt`. Both bindings expose the same grid and damage API, and it has the lower MSRV and a
smaller dependency tree. `ghostty-vt` does have the stronger supply-chain story: an official SHA-256-pinned
release tarball, prebuilt Windows and Apple libraries, and a `search` API. Revisit it if `TermCore::search`
becomes a requirement and MSRV 1.96 is acceptable. The two bindings have nearly identical call shapes, so a
switch would touch roughly 300 lines.

Both dependencies are pinned exactly (`=0.2.2`) because the build script pins the Ghostty commit per release.
A semver-compatible bump would therefore silently change the emulator.

### Licence and build-script review (`libghostty-vt-sys` 0.2.2)

I read the build script from the downloaded `.crate` before building anything. The SHA-256 of both `.crate`
files matches the `Cargo.lock` checksums Cargo used.
- With no overrides, `build.rs` runs `git clone --filter=blob:none https://github.com/ghostty-org/ghostty.git` into
  `OUT_DIR` and checks out the full commit hash `a887df42c56f6de86c0fe6da9c4eeca37931e083`. Git checks the commit
  by its (SHA-1) object hash, so the source is pinned.
- It then runs `zig build -Demit-lib-vt=true -Doptimize=<from profile> -Dcpu=baseline -Dapp-runtime=none`. Zig
  downloads `build.zig.zon` dependencies from `deps.files.ghostty.org` and checks each against its pinned
  multihash. Nothing unpinned or unverified is fetched.
- Optional overrides: `GHOSTTY_SOURCE_DIR` points at a local checkout (no clone), `GHOSTTY_ZIG_SYSTEM_DIR` makes
  Zig resolve packages offline, and `LIBGHOSTTY_VT_SYS_OPTIMIZE` / `LIBGHOSTTY_VT_SYS_CPU` set the optimise mode
  and CPU. Linking is static by default.
- Native code linked into `libghostty-vt.a`, per `src/build/GhosttyLibVt.zig` and `SharedDeps.zig`:
  - Ghostty: MIT.
  - uucode: MIT.
  - simdutf: Apache-2.0 / MIT.
  - Highway: Apache-2.0 / BSD-3-Clause.
  All four are allowed by `deny.toml`.
- `zig build` also downloads the rest of Ghostty's package graph into `~/.cache/zig/p`: freetype, harfbuzz,
  glslang, imgui, oniguruma and others. These downloads are hash-verified and are not linked into lib-vt.
- New crates in `Cargo.lock`: `libghostty-vt`, `libghostty-vt-sys`, `int-enum` (MIT OR Apache-2.0),
  `proc-macro2-diagnostics` (MIT/Apache-2.0), plus the two workspace crates. The `hashbrown` removal in the lock
  diff comes from concurrent manifest edits elsewhere in the workspace, not from this spike.

## Zig

- Version 0.15.2. Ghostty's `build.zig.zon` at the pinned commit declares `minimum_zig_version = "0.15.2"`.
- Installed without sudo. The tarball from `https://ziglang.org/download/0.15.2/zig-x86_64-linux-0.15.2.tar.xz`
  was extracted into a new, empty `~/.local/zig-0.15.2` and symlinked as `~/.local/bin/zig`, which is already on
  `PATH`.
- Verification:
  - The SHA-256 `02aa270f…f93239` matches `https://ziglang.org/download/index.json`.
  - The minisign signature verifies against Zig's published key `RWSGOq2NVecA2UPNdBUZykf1CCb147pkmdtYxgb3Ti+JO/wCYvhbAb/U`.
    minisign is not installed, so I checked it with openssl Ed25519 over the BLAKE2b-512 prehash, covering both the
    file signature and the global trusted-comment signature. A truncated file was correctly rejected.

## MSRV

`libghostty-vt` requires Rust 1.90. Both new crates set `rust-version = "1.90"` locally; the workspace stays at 1.89.

## Design

- `crates/oxt-term-ghostty-sys` holds the only `unsafe` in the backend: one `unsafe impl Send for Vt`. Its
  `Cargo.toml` copies the workspace lints but sets `unsafe_code = "allow"`. It exists because every binding handle
  is `!Send`, while `TermCore: Send + 'static`.
  - `Vt` owns the terminal, the render state, the row and cell iterators, and the `Rc` state its callbacks capture.
  - It is not `Sync`, and its public methods never hand out an owned Ghostty handle or let callers register
    callbacks.
  - At the pinned commit the only `threadlocal` in Ghostty is in `src/crash/sentry.zig`, which lib-vt does not build.
  - The default allocator is libc `malloc`.
  - A test feeds the terminal, moves it to another thread and keeps using it.
- `crates/oxt-term-ghostty` implements `TermCore` with `#![forbid(unsafe_code)]` inherited from the workspace. It
  reuses `oxt_term::input` for every encoder.
  - Feeding uses 16 KiB chunks, with the time budget checked between them.
  - Damage maps Ghostty's `Dirty::Full`/`Partial` and per-row dirty flags. Resize, clear and the first snapshot
    report `full`. `scrolled_rows` is always 0.
- **Ghostty's header documents `max_scrollback` as lines, but `Screen.init` treats it as bytes**, rounded to whole
  pages (fixed ~400 KiB each, whatever the width). The backend converts `scrollback_lines` to bytes as
  `cols·8 + 8` per row, plus about 6% page overhead, plus two pages of slack. Measured retention for 10k requested
  lines: 10,870 at 80 columns and 10,172 at 200. The byte budget is fixed at construction, so after a resize the
  number of lines it buys changes with the width.

### Trait coverage

| `TermCore` item | Status |
|---|---|
| `feed` (byte and time budget, `consumed`) | Supported |
| `drain_replies` | Supported. DA1 answers `CSI ?6c`, like alacritty; DSR 5/6, DECRQM and other replies are Ghostty's own. |
| `input_modes`, `encode_key/mouse/paste/focus` | Supported, through `oxt_term::input`. URXVT and SGR-pixel mouse encodings map to `Legacy`. |
| `resize` (reflow, pixel size for `CSI 14 t`) | Supported |
| `scroll` (Up/Down/Page/Top/Bottom, anchored while scrolled back) | Supported |
| `snapshot` (cells, colours, attributes, wide chars, graphemes, hyperlink flag, cursor, history, damage) | Supported. `BLINK` is now populated. `selection` is always `None`. |
| `take_events`: `Bell`, `Title`, `Cwd` (OSC 7), `ClipboardStore`/`ClipboardStoreRequest` (policy and size cap applied) | Supported |
| `take_events`: `CursorStyle` | Partial. Raised at snapshot time, because the shape is only visible through the render state. |
| `take_events`: `ResetTitle`, `PromptMark` (OSC 133) | Not yet. Ghostty has no callback for them. |
| `memory_estimate` | Approximate. Ghostty does not expose page allocation; within 6% of RSS uncompressed, and it ignores compression. |
| `grapheme` | Supported. Graphemes are interned the same way as in the alacritty backend. |
| `select`, `selection_text` | Not yet. The binding only offers pixel-driven selection gestures. Inert: no-op and `None`. |
| `search` | Not yet. libghostty-vt 0.2 has no search API (`ghostty-vt` does). Always returns `Ok(vec![])`. |
| OSC 52 reads, `answer_clipboard_load` | Not supported. lib-vt drops OSC 52 `?` itself, so reads never reach the embedder. |
| Colour-query replies (OSC 10/11/4 `?`) | Supported. Ghostty answers from the configured palette, byte-identical to alacritty (tested). |
| OSC 133 marks kept off the grid | Supported (tested). No events are raised; see `PromptMark` above. |

### Behaviour differences seen in the parity tests

- An idle snapshot has empty damage. alacritty re-damages the cursor row every time.
- Scrollback cannot be capped at an exact small line count (alacritty's `scrollback_is_capped` uses 10 lines),
  because the limit is bytes and Ghostty prunes whole pages. The ported test checks that at least the configured
  history is kept and that it stays bounded.
- `GhosttyTerm::new` returns `Self` and panics only if Ghostty cannot allocate. `GhosttyTerm::try_new` returns
  `Result`.

## Plugging into `cargo xtask perf`

- Dependency: `oxt-term-ghostty = { path = "../oxt-term-ghostty" }`. There is no cargo feature.
- Constructor: `oxt_term_ghostty::GhosttyTerm::new(config: oxt_term::TermConfig) -> GhosttyTerm`, for example
  `Box::new(GhosttyTerm::new(cfg)) as Box<dyn TermCore>`.
- Optional, not on the trait: `GhosttyTerm::compress_scrollback(&mut self) -> bool`. Call it after feeding to
  measure the compressed per-tab memory.
- Environment:
  - Zig 0.15.2 must be on `PATH`.
  - The first build in each target directory and profile needs network access to `github.com` (git clone) and to
    `deps.files.ghostty.org` (Zig packages, unless `~/.cache/zig/p` already has them). To avoid the clone, set
    `GHOSTTY_SOURCE_DIR` to a checkout of commit `a887df42`.
- Dev-profile builds compile Ghostty in Zig `Debug` mode, which is slow: the 1 MB random-input test takes about
  20 s. Release builds use `ReleaseFast`.

## Risks

- **Maturity:** the binding is third-party and pre-1.0, at its 0.2.x API. The pinned Ghostty commit is a 1.3.2-dev
  `main` snapshot, not a release; libghostty-vt 1.0.1 has shipped since, and `ghostty-vt` tracks it. The header
  documentation is wrong about `max_scrollback` units, which suggests the C API docs lag behind the code.
- **FFI safety:** one `unsafe impl Send`, argued above. It depends on lib-vt never adding thread-affine state;
  re-check the threadlocal grep on every Ghostty bump. All other `unsafe` lives in the binding (about 1k lines
  across render, callbacks and strings). The 1 MB random-input test found no crash, but no fuzzing beyond that was
  done.
- **Build complexity:**
  - Every build machine needs Zig 0.15.x, and the first build needs network access. A transient DNS failure broke
    two of my release builds during the clone.
  - `build.rs` maps the Windows (GNU and MSVC), macOS, Linux (glibc and musl, x86_64 and arm64) and Android
    targets, but **I tested only x86_64 Linux**.
  - Upstream notes Windows link problems: Zig's compiler-rt symbols clash with GNU ld, and MSVC needs lld. The
    `ghostty-vt` fork works around them with prebuilt archives.
  - macOS and arm64 CI are unverified.
- **Zig in CI:**
  - The pipeline must install and verify Zig, ideally with the same checksum and minisign steps.
  - Cache `~/.cache/zig` and the cloned source, or vendor them through `GHOSTTY_SOURCE_DIR` and
    `GHOSTTY_ZIG_SYSTEM_DIR` for offline builds.
  - Until then, `cargo build --workspace` fails on a clean machine without Zig. Verified: in a fresh target
    directory with Zig removed from `PATH`, the build fails with `failed to execute zig build`.
- **Feature gaps:** `TermCore` relies on selection, search, OSC 52 reads and OSC 133 events, and Ghostty does not
  provide them yet. They would need either the newer API (`ghostty-vt`) or our own implementation:
  - OSC 133 and OSC 52: a byte pre-scanner like `oxt-term`'s private `osc_scan`.
  - Selection and search: an extractor based on `grid_ref`.

## Recommendation

Ghostty's core is the only one of the two that can meet both P0 targets: about 660 MB/s against ≥ 300, and
0.5–6.6 MiB per tab against ≤ 3 with compression. I recommend switching the default backend to Ghostty, with
two conditions:

1. **Move to libghostty-vt 1.0.x.** Either adopt `ghostty-vt` (MSRV 1.96; adds search, prebuilt Windows and
   Apple libraries, and SHA-256-pinned release tarballs) or write our own thin `-sys` crate against the 1.0 C
   API with the same tarball-plus-hash scheme. Do not stay on `libghostty-vt` 0.2.2: it tracks a `main`
   snapshot.
2. **Prove CI first.** Add a CI matrix job that builds with Zig on Windows, macOS and Linux arm64. Keep alacritty
   behind the trait as the fallback until selection, search, OSC 52 and OSC 133 are at parity.

Until then, exclude `crates/oxt-term-ghostty` and `crates/oxt-term-ghostty-sys` from the default workspace
members (`default-members` or `exclude`), so builds without Zig keep working.

## Files

- `crates/oxt-term-ghostty-sys/` (`Cargo.toml`, `src/lib.rs`): the `Send` owner, with a thread-move test.
- `crates/oxt-term-ghostty/` (`Cargo.toml`, `src/lib.rs`, `tests/term_core.rs`, `examples/backend_compare.rs`):
  the backend, the parity tests and the measurement example.
- `Cargo.lock`: adds the packages listed above.
- `~/.local/zig-0.15.2`, `~/.local/bin/zig`: the Zig install.

## Unresolved questions

- Should the follow-up target `ghostty-vt` (MSRV 1.96) or our own 1.0 `-sys` crate?
- Is idle compression acceptable as the way to meet ≤ 3 MiB, given that real logs compress less than the test
  history?
- Ghostty reports SGR 2 only as the `DIM` flag and leaves the colour as is. I did not check whether the alacritty backend also dims the colour (`oxt-term`'s palette has dim entries). Which layer should apply dimming?
