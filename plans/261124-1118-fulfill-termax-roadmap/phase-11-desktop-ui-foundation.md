---
title: "Phase 11: Desktop UI Foundation"
status: todo
priority: P1
effort: "10–12 person-weeks"
dependencies: [1, 3]
---

# Phase 11: Desktop UI Foundation

## Overview

Decide the UI framework with the bake-off and build the app shell, design system and GPU terminal renderer.

Owner: UI lead · Priority: P1 · Effort: 10–12 person-weeks · Depends on: phase 1, phase 3

## Requirements

- [ ] D1 bake-off gate (ARCH-BRIEF §2): ≥16 panes @60 fps, idle CPU <0.5 %, idle RSS <150 MB, IME on Wayland + Windows, VoiceOver/Narrator
- [ ] `app-shell-design-system` UI-01 (12), UI-10 (12)
- [ ] `terminal-workspace` CO-05 GPU renderer (9 P1, 1 P2)

## Implementation Steps

1. 3-week bake-off on a spike branch (GPUI vs Tauri); report with measured gate results; owner decides D1 (phase 1).
2. Change `add-app-shell` (UI-01) and `add-design-system-and-settings` (UI-10, VI/EN with Fluent, AccessKit).
3. Change `add-terminal-renderer` (CO-05) consuming `Frame`/`Damage`.

## Todo

- [ ] Bake-off report
- [ ] D1 decided
- [ ] UI-01 merged
- [ ] UI-10 merged
- [ ] CO-05 merged

## Success Criteria

- UI perf metrics added to xtask (frame time, idle wakeups)
- a11y checks (AccessKit tree tests)
- `cargo xtask spec-coverage` for UI-01/UI-10/CO-05
