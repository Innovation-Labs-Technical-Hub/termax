# Terminal core and SSH performance baseline (2026-11-24)

Machine: developer Linux workstation, release build (`cargo xtask perf`, 64 MiB per workload, best of 3).

| Metric | Measured | Target (ARCH-BRIEF §6) | Result |
|---|---|---|---|
| Parser, plain ASCII | 158.5 MB/s | ≥ 300 MB/s | miss |
| Parser, SGR-heavy | 135.6 MB/s | – | info |
| Parser, Vietnamese/CJK/emoji | 117.7 MB/s | – | info |
| Snapshot, full 200×60 | 0.051 ms | ≤ 1.5 ms (incl. GPU share) | pass |
| Snapshot, one line 200×60 | 0.002 ms | ≤ 0.2 ms | pass |
| Memory per 80×24 tab, 10k lines (RSS) | 18.9 MiB | ≤ 3 MiB | miss |
| SSH connect, loopback (TCP + ML-KEM KEX + key auth) | 26.4 ms | – | info |
| SSH new channel + exec on existing connection | 1.6 ms | ≤ 20 ms + network | pass |
| SSH channel throughput, 128 MiB, loopback | 986 MB/s (OpenSSH client: 651 MB/s) | ≥ 0.8× OpenSSH | pass (1.51×) |

## Where the gaps come from
- **Throughput:** raw `alacritty_terminal` 0.26 measured in isolation on the same input reaches 177 MB/s with 10k lines of scrollback and 232 MB/s with none. The Termax wrapper (4 KiB chunks for the time budget, OSC 7/133 pre-scanner) costs about 11 %. The 300 MB/s target is therefore above what the chosen backend delivers on this machine, not a wrapper problem.
- **Memory:** alacritty stores about 24 bytes per cell, so 10k × 80 cells ≈ 19 MB, exactly as analysis/02-terminal-emulation.md §7 predicted. Meeting 3 MiB needs the tiered scrollback the analysis proposes (hot ring + compressed cold pages) or a backend with packed cells (libghostty-vt uses 8-byte cells).

## Options
1. Keep alacritty and relax the targets for P1 (e.g. ≥150 MB/s, ≤20 MiB per 10k-line tab).
2. Keep alacritty but cap default scrollback lower and add compressed cold scrollback (fork/upstream work).
3. Run the libghostty-vt spike behind the same `TermCore` trait and compare with this harness.

CI runs the harness report-only until one of these is chosen.

## Update 2026-11-24 13:40: harness v2 and backend comparison

The harness now separates targets from regressions (`--baseline`, 10 % tolerance on stable metrics; RSS is reported but not gated because allocator reuse makes it vary by more than 100 % between identical runs). New metrics: echo latency under a flood behind a bounded SSH window, and per-tab memory for empty, 1k and 10k-line history.

`just ghostty-compare` on the same machine:

| Metric | alacritty_terminal | libghostty-vt | Target |
|---|---|---|---|
| Parser, plain ASCII | 151 MB/s | 665 MB/s | ≥ 300 |
| Snapshot, full 200×60 | 0.050 ms | 0.262 ms | ≤ 1.5 |
| Snapshot, one line | 0.002 ms | 0.008 ms | ≤ 0.2 |
| Echo under flood p99, 1 MiB window | 24.1 ms | 18.2 ms | ≤ 50 |
| Echo under flood p99, 4 MiB window | 47.5 ms | 22.9 ms | ≤ 50 |
| Memory per tab, empty history (RSS) | 0.03 MiB | 0.17 MiB | – |
| Memory per tab, 10k lines (RSS) | 17.2 MiB | 6.5 MiB (0.53 MiB after idle compression, best case) | ≤ 3 |

Findings:
- The user-facing requirement (typed input visible within 50 ms during a flood) is met by both backends, but alacritty has almost no headroom at a 4 MiB window.
- Idle tabs are cheap on both; the memory target only bites for tabs whose 10k-line history is full.
- Ghostty meets the throughput target; it meets the memory target only with idle scrollback compression. Its gaps (selection, search, OSC 52 reads, OSC 133 marks) are listed in `ghostty-spike-261124-backend-comparison.md`.
