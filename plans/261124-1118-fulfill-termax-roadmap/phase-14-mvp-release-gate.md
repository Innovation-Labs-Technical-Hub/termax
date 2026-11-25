---
title: "Phase 14: MVP Release Gate"
status: todo
priority: P1
effort: "3–4 person-weeks"
dependencies: [3, 4, 5, 6, 7, 8, 9, 12, 13]
---

# Phase 14: MVP Release Gate

## Overview

Prove the desktop MVP meets its requirements and NFRs before the beta.

Owner: Tech lead + Product owner · Priority: P1 · Effort: 3–4 person-weeks · Depends on: phase 3, phase 4, phase 5, phase 6, phase 7, phase 8, phase 9, phase 12, phase 13

## Requirements

- [ ] All P1 `Must` requirements (342 P1 functional requirements in total)
- [ ] NFR-00-* and per-area NFR targets (performance, idle, memory, latency, a11y, i18n VI+EN)
- [ ] Crash-free sessions ≥99.5 % in beta

## Implementation Steps

1. Run `cargo xtask spec-coverage --phase P1` and close gaps.
2. Run `cargo xtask perf --check` on reference machines per OS.
3. Security review and red-team of agent features.
4. Beta release with opt-in crash reporting.

## Todo

- [ ] P1 Must coverage = 100 %
- [ ] `perf --check` passes on reference machines
- [ ] Security sign-off
- [ ] Beta shipped

## Success Criteria

- Coverage report attached to the release
- Perf JSON per OS attached
- Beta crash-free metric tracked
