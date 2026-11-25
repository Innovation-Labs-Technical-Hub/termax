---
title: Roadmap plan to fulfill the Termax checklist
date: 2026-11-24
summary: "19-phase plan covering owner decisions, P1 MVP, release, and P2/P3 scope"
---

# Roadmap plan to fulfill the Termax checklist

## What happened
Turned the remaining-work checklist into `plans/261124-1118-fulfill-termax-roadmap` (19 phases, 139 tasks), committed on branch `plan/fulfill-checklist` (7d25a57), not pushed. `ak plan validate` passes.

## Decision
- Each phase is delivered as one or more OpenSpec changes on its own branch/PR, gated by `just ci`, `just spec-validate` and the CI perf gate.
- Owner decisions (D1–D9, 71 BRD open questions, FR-CO01-011 quiet period, `.oxtpkg`/`.oxpart` names) are isolated in phase 1 and never assumed.
- Phase 2 adds `cargo xtask spec-coverage` so progress is measured as linked, passing acceptance scenarios.
- Ghostty on macOS/Windows is tracked separately (phase 10) because it depends on upstream Zig/Ghostty.
- An autonomous optimization loop was not used: the checklist has no single metric and many items are blocked on owner decisions.

## Next steps
Start phases 1, 2 and 10 in parallel; then phase 3 (`add-session-runtime`). Recalibrate the ≈130–150 person-week estimate after phase 3. Plan validation questions were not asked because the question dialog was declined.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
