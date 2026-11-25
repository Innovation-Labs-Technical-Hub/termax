# Ghostty (libghostty-vt) on macOS 26 and Windows (VS 2026): research for Phase 10

Date: 2026-11-24 (Asia/Saigon). Machine: Linux x86_64, rustc 1.99.1. Scratch: `/tmp/claude-1000/-home-tonythai93-Projects-termax/44af4a3b-5c6d-4e3d-8cae-7b5beef43bcd/scratchpad/phase10/`.

## Outcome

Neither failure needs a Termax-owned fork. Both are fixed by Zig 0.16 or later, but our pinned Ghostty commit cannot build with any Zig newer than 0.15.2. Fixing Phase 10 therefore means moving to Ghostty `vt-1.0.x`, which requires Zig 0.17.x and a different Rust binding.

**Recommendation:** switch to `ghostty-vt` 0.9.0+ghostty-vt.1.0.1 with Zig 0.17.1 on all five CI platforms, and raise the workspace MSRV from 1.90 to 1.96. Use the prebuilt XCFramework on macOS and a source build with Zig 0.17.1 on Windows MSVC. Set the clang `CCC_OVERRIDE_OPTIONS` define only in the Windows jobs, as a guard against the next MSVC Clang-version bump. Move to the upstream `libghostty-vt` crate once it publishes a 1.0 release (issue Uzaaft/libghostty-rs#135).

- Effort: about 1–1.5 person-weeks.
- Main risks: the binding is a single-maintainer fork, the MSRV bump, and Windows arm64 MSVC is not tested upstream.

Keeping 0.2.2 with Zig 0.15.2 and patching CI is possible, but it is fragile and only works in CI. It also leaves the pinned commit without Windows fixes that the memory target depends on (see Q3).

## Q1. Newer Zig releases

Source: `curl https://ziglang.org/download/index.json` (fetched 2026-11-24) and the release notes at `ziglang.org/download/<v>/release-notes.html`.

| Zig | Date | Bundled clang/LLVM | macOS 26 SDK (fix for zig#31658) | MSVC 14.52 STL (needs Clang ≥ 22) |
|---|---|---|---|---|
| 0.15.2 (ours) | 2025-10-11 | clang 20.1.2 (`zig cc --version`, local) | broken | broken (STL1000) |
| 0.16.0 | 2026-04-13 | LLVM 21.1.0, `zig cc` = Clang 21.1.8; macOS 26.4 headers | **fixed** (milestone 0.16.0) | broken: Clang 21 < 22 (seen in herdr-core#361 and with-hide/hide#2644) |
| 0.17.0 | 2026-10-01 | LLVM 22.1.8, `zig cc` = Clang 22.1.8; macOS 27.0 headers; COFF linker can link both -gnu and -msvc libc | fixed | **passes the gate** (22 ≥ 22) |
| 0.17.1 | 2026-11-02 | same as 0.17.0 (local `zig cc --version` prints `clang version 22.1.8`) | fixed | passes |
| 0.18.0-dev | master | LLVM 23 planned | n/a | n/a |

- **macOS.** Our CI failure is Zig issue [codeberg ziglang/zig#31658](https://codeberg.org/ziglang/zig/issues/31658), "MacOS Builds Are Failing w/ XCode 26.4". It shows the same symbols we hit (`__availability_version_check` from compiler_rt, `_abort`, `_arc4random_buf` and `_bzero` from `build_zcu.o`).
  - It was closed on 2026-03-27 with milestone 0.16.0 and fixed by [zig PR 31673](https://codeberg.org/ziglang/zig/pulls/31673). There is no 0.15.x backport release.
  - Cause: the `.tbd` stubs in the macOS 26.4+ SDK list only `arm64e-macos`, while Zig 0.15.2's Mach-O linker matches `arm64`/`arm64-macosx` ([towles-tool#829](https://github.com/ChrisTowles/towles-tool/pull/829), [betamax#280](https://github.com/joshka/betamax/pull/280)).
  - The failing object is `build_zcu.o`, which is the build runner itself, so `zig build` cannot even start. Our log confirms it (job 138289239727, image `macos-26-arm64` 20261117.0516.1).
- **Windows.** [microsoft/STL `yvals_core.h`](https://github.com/microsoft/STL/blob/main/stl/inc/yvals_core.h) contains `#if __clang_major__ < 22 … STL1000 "expected Clang 22 or newer"`, guarded by `#ifndef _ALLOW_COMPILER_AND_STL_VERSION_MISMATCH`. The minimum went up in the STL toolset commit of 2026-05-28 ("Clang 22.1.3", #6296), and runner image `windows-2025-vs2026` 20261116.315 shipped it as MSVC 14.52.37025.
  - The `-nostdinc++` and `-fno-rtlib-defaultlib` "argument unused" lines and the simdutf array-bounds lines are warnings. Zig prints them only because the compile failed. Evidence: with-hide/hide#2646 says so, and Ghostty's own MSVC CI passes while Zig still adds `-nostdinc++`.

## Q2. Ghostty and the Rust bindings

**Ghostty's Zig requirement by ref.** I read `minimum_zig_version` from `build.zig.zon` at each ref with `gh api`.

| Ref | Date | Ghostty version | Zig |
|---|---|---|---|
| `a887df42` (our pin) | 2026-07-11 | 1.3.2-dev | 0.15.2 |
| `e8525c0f` "Update to Zig 0.16.0" | 2026-07-21 | | 0.16.0 |
| PR #14519 "Update to Zig 0.17" | 2026-10-20 | | 0.17.0 |
| v1.4.0 | 2026-10-27 | | 0.17.0 |
| **vt-1.0.0** | 2026-11-06 | | 0.17.0 |
| **vt-1.0.1** (`108cacb1`) | 2026-11-11 | | 0.17.0 |
| main | 2026-11-14 | 1.4.1-dev | 0.17.1 |

**Ghostty's own platform support** (`.github/workflows/test.yml` on main; latest green main run 45313711180, 2026-11-24):
- **Windows.** The `build-libghostty-vt-windows` job runs `zig build test-lib-vt`, `zig build -Demit-lib-vt`, static and DLL examples with Zig 0.17.1. It runs natively on a Namespace Windows runner, and Ghostty defaults the native Windows ABI to **msvc** (`src/build/Config.zig`: "Zig defaults to the GNU ABI which produces objects with invalid COMDAT sections that MSVC rejects (LNK1143)").
  - Cross builds cover `x86_64-windows-gnu` and `aarch64-windows-gnu`. Cross-compiling `x86_64-windows-msvc` is commented out ("we need a way to find msvc libc/c++ headers").
  - The runner's MSVC version is not shown in the log, so it is **unverified** whether that CI already runs against MSVC 14.52.
  - No aarch64-windows-msvc job exists.
- **macOS.** Jobs use Xcode 27.1 (`build-libghostty-vt-macos`, XCFramework).
  - [PR 12417](https://github.com/ghostty-org/ghostty/pull/12417) (2026-04-24) lets lib-vt cross-compile for macOS from Linux without an SDK.
- **Release assets.** [PR 15151](https://github.com/ghostty-org/ghostty/pull/15151) (2026-11-08) publishes prebuilt Windows zips at `release.files.ghostty.org/libghostty-vt/1.0.1/`. HTTP checks show the XCFramework and `x86_64`/`aarch64-windows-gnu` zips → 200, and `x86_64-windows-msvc` → 404. **The Windows prebuilts are GNU-ABI only.**
- **Fixes after our pin that matter to us:**
  - #13452, static lib-vt linking on Windows (2026-07-26).
  - `84254a9d` "avoid MSVC C++ runtime in no-libcxx builds" (2026-07-26).
  - #14190, "reclaim pooled and compressed page memory on Windows" (2026-09-09), needed for the NFR-00-003 memory target on Windows.
  - #14447, DLL global constructors.

**Crates** (crates.io API, 2026-11-24):

| Crate | Latest | Ghostty | Zig | MSRV | macOS 26 / Windows MSVC |
|---|---|---|---|---|---|
| `libghostty-vt(-sys)` | 0.2.2 (2026-09-28); no newer release | `a887df42` | 0.15.2 | 1.90 | both fail (our CI) |
| libghostty-rs `master` (unreleased) | Cargo.toml still says 0.2.1 | vt-1.0.1 (`108cacb1`, PR #131) | 0.17.x | **1.96** (PR #124) | source build: same as Ghostty upstream |
| `ghostty-vt(-sys)` | 0.9.0+ghostty-vt.1.0.1 (2026-11-14) | vt-1.0.1 release tarball, SHA-256 pinned (`GHOSTTY.lock`) | 0.17.x (build script refuses other versions) | **1.96** | see below |

How `ghostty-vt-sys` 0.9.0 links on each target (I read its `build.rs` and README from the downloaded `.crate`, sha256 `c76694a0…`):
- **macOS/iOS:** links the release's prebuilt XCFramework, so the build needs **no Zig**. It never touches Zig's linker, which avoids zig#31658 entirely.
- **`*-windows-gnu`/`gnullvm`:** links the prebuilt GNU zip and renames compiler_rt symbols in it.
- **`*-windows-msvc`:** builds from source with Zig 0.17 by default, which needs a Windows host and the Windows SDK. `GHOSTTY_VT_PREBUILT_MSVC=1` links the GNU zip instead but needs lld; the build stops if the linker is `link.exe` (LNK1143).
- **Linux:** source build.
- **Other knobs:** `GHOSTTY_VT_LIB_DIR`, `GHOSTTY_VT_PREBUILT_URL` + SHA, `GHOSTTY_SOURCE_DIR`, `GHOSTTY_ZIG_SYSTEM_DIR`, a `pkg-config` feature, and `ZIG`.

Adoption signals:
- `ghostty-vt`: 7,851 downloads, 2 reverse dependencies, single maintainer (repo `prabirshrestha/gpui-cn`).
- `libghostty-vt`: 216,548 downloads, 15 reverse dependencies, two maintainers who say they are "cooking" a 1.0 release (issue #135, 2026-11-17, no date given).

## Q3. Workarounds, ranked

| # | Option | Feasible? | What Termax owns | Licence | CI cost | Verdict |
|---|---|---|---|---|---|---|
| A | **Move to `ghostty-vt` 0.9.0 (vt-1.0.1) + Zig 0.17.1** | Yes. Verified on Linux (below); macOS/Windows **unverified in CI** | Binding swap of about 300 lines in `termax-term-ghostty-sys`; re-check the `Send` argument; MSRV 1.96; add Zig 0.17.1 to CI | Apache-2.0. `cargo deny` with our `deny.toml` fails only on `CDLA-Permissive-2.0` (`webpki-roots`/`webpki-root-certs` via build-time `ureq`), so that licence must be allowed | Zig setup on 5 runners. macOS needs no Zig build (about 0 s); Windows MSVC builds from source (about 1–2 min, estimated from local 50–110 s builds) | **Recommended** |
| B | Wait for an upstream `libghostty-vt` 1.0 release (or git-pin libghostty-rs master) | Yes, once it is published | Same swap, closer to today's API; git pin until then | MIT OR Apache-2.0 | Same as A, but macOS builds from source with Zig 0.17 (fixed SDK handling) | Use as the follow-up to A, or instead of A if the owner refuses a single-maintainer fork |
| C | Keep 0.2.2 / Zig 0.15.2. macOS: `DEVELOPER_DIR=/Applications/Xcode_26.3.app` (SDK 26.2). Windows: inject the STL define (below) | CI only. Workaround reports in zig#31658; the `macos-26` image has Xcode 26.3 with SDK 26.2. Windows define **unverified with clang 20** (hide verified it with clang 21) | CI env only, no code | none | ~0 | Stopgap only. Developers on Xcode ≥ 26.4 or VS 2026 still fail; Windows lacks #14190 (memory target) and the post-pin MSVC fixes |
| D | `_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH` without patching Ghostty | **Yes.** Zig's clang honours `CCC_OVERRIDE_OPTIONS`, also inside `zig build` (verified below) | One env var on the Windows jobs, e.g. `CCC_OVERRIDE_OPTIONS=+-D_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH` | none | 0 | Keep it as a guard together with A: MSVC raises its minimum Clang about once a year and Zig trails it (hide#2646 makes the same argument) |
| E | Target `*-windows-gnu` with Zig's bundled libc++/MinGW | The lib builds (verified, both Zig versions) | Either ship the Rust `x86_64-pc-windows-gnu(llvm)` target for the whole app, or link the GNU archive into MSVC Rust with lld (`link.exe` → LNK1143) | none extra | Toolchain change for all of Termax; aws-lc-sys on the gnu target is untested | Reject as the default; only `ghostty-vt`'s opt-in `GHOSTTY_VT_PREBUILT_MSVC` + lld path is worth keeping in mind |
| F | Prebuild libghostty-vt per platform ourselves and link via pkg-config / `GHOSTTY_VT_LIB_DIR` | Cross-compile from Linux works for macOS and windows-gnu (verified). MSVC needs a Windows host | A release pipeline, artefact hosting and SHA pins, ABI matching to the bindings | Ghostty is MIT; redistributing binaries means shipping its licence and third-party notices | An extra workflow per Ghostty bump | Not needed: upstream already publishes these artefacts, and `ghostty-vt-sys` consumes them |
| G | Pass an older SDK, `-Dtarget` or sysroot to Zig 0.15.2 on macOS | Not as a fix: the failure is in linking the build runner against the host SDK, before `-Dtarget` matters | n/a | n/a | n/a | Reject (C is the only variant that works) |

The plan's step 2, "own build script with newer Zig, link via pkg-config", only works with a binding that matches the vt-1.0 C API. Pointing 0.2.2's checked-in bindings at a vt-1.0.1 build is not viable, because libghostty-rs needed breaking binding changes for 1.0 (PRs #125 and #129).

## Q4. Local evidence (Linux, this machine)

Commands ran from the scratch directory; the downloads above went into separate new directories.

1. **Zig 0.17.1.** Downloaded `zig-x86_64-linux-0.17.1.tar.xz`. `sha256sum -c` against the `index.json` value `3581863e…b17a` printed OK. `zig version` printed 0.17.1 and `zig cc --version` printed clang 22.1.8. Zig 0.15.2 printed clang 20.1.2.
2. **libghostty-vt 1.0.1 release tarball and deps tarball.** Both SHA-256 values match `ghostty-vt-sys`'s `GHOSTTY.lock`. Build command: `zig build -Demit-lib-vt=true -Doptimize=ReleaseFast -Dcpu=baseline -Dapp-runtime=none -Demit-xcframework=false -Dtarget=<t> --system <deps>`, with `ZIG_GLOBAL_CACHE_DIR` set because 0.17 removed `--global-cache-dir`:
   - native x86_64-linux: OK in 112 s (`libghostty-vt.a` and `.so.1.0.1`).
   - `aarch64-macos` and `x86_64-macos`: OK (`.a` and `.dylib`), with no SDK.
   - `x86_64-windows-gnu` and `aarch64-windows-gnu`: OK (`ghostty-vt-static.lib`, DLL, import lib).
   - `x86_64-windows-msvc` and `aarch64-windows-msvc`: fail with `'stdlib.h'`/`'cstring' file not found`. No MSVC headers exist on Linux, which matches Ghostty's own CI note. **The STL1000 gate cannot be reproduced here.**
3. **Pinned `a887df42` with Zig 0.15.2.** `aarch64-macos`, `x86_64-windows-gnu` and `aarch64-windows-gnu` all build OK (about 40 s each). msvc fails for the same missing-headers reason. The macOS link problem is host-side only, so it cannot be reproduced on Linux.
4. **Define injection.**
   - Plain clang test: `CCC_OVERRIDE_OPTIONS="+-D_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH" zig c++ -c t.cpp` makes a file that has `#error` when the define is missing compile under both 0.15.2 and 0.17.1. Clang logs `### Adding argument … at end`.
   - Propagation through `zig build`: a deliberately broken override (`+-include +/nonexistent-probe.h`) made `compile lib simdutf` fail with that include. So the env var reaches every C/C++ compile without patching Ghostty.
   - Caveat: the variable is not part of Zig's cache key, so use a fresh cache dir. Build scripts get a fresh `OUT_DIR`, so they already do.
5. **New binding end to end.** A scratch crate depending on `ghostty-vt = "=0.9.0"` built with `ZIG=<0.17.1> cargo run --release` (117 s cold), created a `Terminal`, wrote SGR text and ran fine.
   - `cargo deny check licenses bans` with Termax's `deny.toml`: bans OK, licences failed only on CDLA-Permissive-2.0 (webpki-roots).
6. **Thread affinity.** In the vt-1.0.1 source, `grep threadlocal src/**/*.zig` still finds only `crash/sentry.zig` (plus a comment in the OpenGL renderer). The `Send` soundness premise therefore still holds, but must be re-checked against the new binding's handle types.

Not tested: linking on a real macOS 26 or VS 2026 host, Windows arm64 MSVC, and whether the macOS archive's undefined symbols resolve under Apple ld (no Mach-O `nm` on this machine).

## Q5. Recommended steps (option A, plus D as a guard)

1. **Decisions** (owner or tech lead): raise the MSRV to 1.96 (`.principles/rust.md` requires a reason in the commit body), allow `CDLA-Permissive-2.0` in `deny.toml` (permissive data licence for CA roots, used only at build time), and accept the single-maintainer `ghostty-vt` crate.
2. **Swap the binding.** In `termax-term-ghostty-sys`, replace `libghostty-vt`/`-sys =0.2.2` with `ghostty-vt`/`-sys =0.9.0+ghostty-vt.1.0.1` (about 300 lines per the spike). Re-check the `Send` argument and the scrollback unit: 0.9.0 still exposes `max_scrollback: usize`, and whether it is bytes or lines is unverified. Run the 28 parity tests and `backend_compare`.
3. **CI.** Install Zig 0.17.1 on all five runners (index.json has `aarch64-windows`, `aarch64-macos` and the others), set `ZIG`, remove the Ghostty `--exclude` in `ci.yml`, and set `CCC_OVERRIDE_OPTIONS=+-D_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH` on the Windows jobs only. Do not set it in `.cargo/config.toml [env]`: clang writes `###` lines to stderr on every compile, which can confuse `cc`-crate flag probes on other platforms.
4. **Docs.** Update `justfile` `doctor`/`setup` (Zig 0.15.2 → 0.17.1), the crate `AGENTS.md` files, the spike report and the platform note in `openspec/config.yaml`.
5. **Flip `DEFAULT_BACKEND`** for macOS and Windows only after `just term-test` and `compare-backends` pass on each platform (Phase 10 success criteria).

Effort: about 2 days for the binding swap and tests, 1–2 days of CI iteration (Windows arm64 is the unknown), and 1 day for per-platform perf numbers and docs. That fits inside the plan's 2–3 person-week budget.

Risks:
- Single-maintainer fork. Mitigation: plan B, upstream `libghostty-vt` 1.0, has a near-identical API.
- MSVC raising its Clang minimum again. Mitigation: D.
- Untested aarch64-windows-msvc. Fallback: `GHOSTTY_VT_PREBUILT_MSVC=1` with `rust-lld`.
- Build-time network fetch from `release.files.ghostty.org`. It is SHA-pinned, and the build can run offline via `GHOSTTY_RELEASE_DIR`/`GHOSTTY_VT_LIB_DIR`.
- Zig 0.17 keeps loop vectorization disabled (release notes). Re-measure parser throughput.

Upstream items to watch:
- [libghostty-rs#135](https://github.com/Uzaaft/libghostty-rs/issues/135) (1.0 release).
- Ghostty `release-lib-vt.yml`: whether an MSVC zip is added.
- Ghostty adding `_ALLOW_COMPILER_AND_STL_VERSION_MISMATCH` upstream.
- Zig 0.18 (LLVM 23).
- The next MSVC STL toolset update in microsoft/STL.

## Sources

- Zig: [download index](https://ziglang.org/download/index.json), [0.16.0 notes](https://ziglang.org/download/0.16.0/release-notes.html), [0.17.0 notes](https://ziglang.org/download/0.17.0/release-notes.html), [zig#31658](https://codeberg.org/ziglang/zig/issues/31658).
- MSVC: [microsoft/STL yvals_core.h](https://github.com/microsoft/STL/blob/main/stl/inc/yvals_core.h), [actions/runner-images Windows 2025 VS2026 readme](https://github.com/actions/runner-images/blob/main/images/windows/Windows2025-VS2026-Readme.md), [macOS 26 arm64 readme](https://github.com/actions/runner-images/blob/main/images/macos/macos-26-arm64-Readme.md).
- Ghostty: [PR 14519 Zig 0.17](https://github.com/ghostty-org/ghostty/pull/14519), [PR 15151 Windows zips](https://github.com/ghostty-org/ghostty/pull/15151), [PR 12417 macOS cross](https://github.com/ghostty-org/ghostty/pull/12417), [PR 14190 Windows page reclaim](https://github.com/ghostty-org/ghostty/pull/14190), [open PR 13151 MSVC CRT](https://github.com/ghostty-org/ghostty/pull/13151).
- Bindings: [Uzaaft/libghostty-rs](https://github.com/Uzaaft/libghostty-rs), [crates.io ghostty-vt](https://crates.io/crates/ghostty-vt), [crates.io libghostty-vt](https://crates.io/crates/libghostty-vt).
- Downstream reports: [with-hide/hide#2644](https://github.com/with-hide/hide/issues/2644) and PRs #2646/#2656, [herdrdev/herdr-core#361](https://github.com/herdrdev/herdr-core/issues/361), [ai-sirio/sirio#805](https://github.com/ai-sirio/sirio/issues/805), [ChrisTowles/towles-tool#829](https://github.com/ChrisTowles/towles-tool/pull/829), [joshka/betamax#280](https://github.com/joshka/betamax/pull/280).
- Termax CI probe: run 45348627627 (jobs 138289239639, 138289239566, 138289239727).

Credibility: Zig release notes, the Zig tracker, the microsoft/STL source, Ghostty source and CI, and the crate source are primary. The downstream issues are independent production reports that agree with each other.

## Unresolved questions

1. Will the owner accept MSRV 1.96 and a single-maintainer fork (A), or prefer to wait for upstream `libghostty-vt` 1.0 (B) with no release date?
2. Is `CDLA-Permissive-2.0` acceptable in `deny.toml`, given it is only a build-time dependency?
3. Does aarch64-windows-msvc build from source with Zig 0.17.1 on `windows-11-arm`? Neither Ghostty nor `ghostty-vt` CI covers it. A one-job CI probe would answer it.
4. Does the XCFramework, built with Xcode 27.1, link cleanly with the Xcode 26.x linker on `macos-latest` without "built for newer macOS" warnings? This is unverified.
