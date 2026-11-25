---
title: "Roadmap phases 1, 2 and 10 started"
date: 2026-11-24
summary: "Decision pack, spec-coverage tooling (24/1547 baseline) and Ghostty macOS/Windows path found; blocked on owner decisions"
---

# Roadmap phases 1, 2 and 10 started

## What happened
- Phase 1: decision pack (D1–D9, 71 BRD questions by owner, 13 spec-vs-code differences, GH-1…3) on PR #4.
- Phase 2: `cargo xtask spec-coverage`, 44 tagged tests, library lints, idle-compaction memory metric, CI coverage job — PR #5, CI green on all 9 jobs. Baseline 24/1547 scenarios (P1 24/1055).
- Phase 10: Zig 0.17.x fixes macOS 26 linking (Zig #31658) and the VS 2026 "Clang 22" STL check; needs Ghostty vt-1.0.1 via `ghostty-vt` 0.9.0 (Rust 1.96, CDLA-Permissive-2.0 build dep).

## Decision
No owner decision was assumed; MSRV 1.96, the licence and the single-maintainer crate are recorded as GH-1…GH-3.

## Next steps
Owner decisions (D1 first, GH-1…3, the 13 differences), then phase 3 (session runtime) and the phase 10 binding swap.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
