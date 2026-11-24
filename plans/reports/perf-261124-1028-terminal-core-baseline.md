# Terminal core performance baseline (2026-11-24)

Machine: developer Linux workstation, release build (`cargo xtask perf`, 64 MiB per workload, best of 3).

| Metric | Measured | Target (ARCH-BRIEF §6) | Result |
|---|---|---|---|
| Parser, plain ASCII | 158.5 MB/s | ≥ 300 MB/s | miss |
| Parser, SGR-heavy | 135.6 MB/s | – | info |
| Parser, Vietnamese/CJK/emoji | 117.7 MB/s | – | info |
| Snapshot, full 200×60 | 0.051 ms | ≤ 1.5 ms (incl. GPU share) | pass |
| Snapshot, one line 200×60 | 0.002 ms | ≤ 0.2 ms | pass |
| Memory per 80×24 tab, 10k lines (RSS) | 18.9 MiB | ≤ 3 MiB | miss |

## Where the gaps come from
- **Throughput:** raw `alacritty_terminal` 0.26 measured in isolation on the same input reaches 177 MB/s with 10k lines of scrollback and 232 MB/s with none. The OxTerm wrapper (4 KiB chunks for the time budget, OSC 7/133 pre-scanner) costs about 11 %. The 300 MB/s target is therefore above what the chosen backend delivers on this machine, not a wrapper problem.
- **Memory:** alacritty stores about 24 bytes per cell, so 10k × 80 cells ≈ 19 MB, exactly as analysis/02-terminal-emulation.md §7 predicted. Meeting 3 MiB needs the tiered scrollback the analysis proposes (hot ring + compressed cold pages) or a backend with packed cells (libghostty-vt uses 8-byte cells).

## Options
1. Keep alacritty and relax the targets for P1 (e.g. ≥150 MB/s, ≤20 MiB per 10k-line tab).
2. Keep alacritty but cap default scrollback lower and add compressed cold scrollback (fork/upstream work).
3. Run the libghostty-vt spike behind the same `TermCore` trait and compare with this harness.

CI runs the harness report-only until one of these is chosen.
