---
title: "Phase 12: Desktop UI Feature Screens"
status: todo
priority: P1
effort: "20–24 person-weeks"
dependencies: [11, 4, 5, 6, 7, 8]
---

# Phase 12: Desktop UI Feature Screens

## Overview

Build every MVP screen on top of the core crates.

Owner: UI lead + UI engineer · Priority: P1 · Effort: 20–24 person-weeks · Depends on: phase 11, phase 4, phase 5, phase 6, phase 7, phase 8

## Requirements

- [ ] UI-02 hosts home (11), UI-03 host editor (12), UI-04 terminal workspace (15), UI-05 SFTP dual pane (12)
- [ ] UI-06 port forwarding (8), UI-07 keychain/known hosts/trust sheet (11), UI-08 snippets/logs (7 P1, 2 P2, 1 P3), UI-09 AI panel and approvals (10)
- [ ] Mockups MK-01…MK-10 in the design set

## Implementation Steps

1. One OpenSpec change per screen, in dependency order: UI-02/03 → UI-04 → UI-07 → UI-06 → UI-05 → UI-08 → UI-09.
2. Each screen ships with keyboard-only paths, screen-reader labels and VI/EN strings.

## Todo

- [ ] UI-02/03 merged
- [ ] UI-04 merged
- [ ] UI-05 merged
- [ ] UI-06 merged
- [ ] UI-07 merged
- [ ] UI-08 merged
- [ ] UI-09 merged

## Success Criteria

- UI tests per screen
- a11y audit per screen (WCAG 2.2 AA)
- `cargo xtask spec-coverage` for UI-02…UI-09
