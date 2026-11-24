---
title: "Phase 1: Owner Decisions and Repository Governance"
status: in-progress
priority: P1
effort: "1–2 weeks elapsed (owner time)"
dependencies: []
---

# Phase 1: Owner Decisions and Repository Governance

## Overview

Collect the decisions only owners can make, so later phases are not built on assumptions, and put repository governance in place.

Owner: Product owner + Tech lead · Priority: P1 · Effort: 1–2 weeks elapsed (owner time) · Depends on: nothing

## Requirements

- [ ] ARCH-BRIEF decisions D1–D9 (`openspec/config.yaml` → Open decisions)
- [ ] 71 BRD open questions marked `[TO CONFIRM · CHỜ XÁC NHẬN]` across `openspec/specs/*/spec.md`
- [ ] FR-CO01-011 quiet period before idle compaction (change `add-session-runtime`)
- [ ] Codename-derived extensions `.oxtpkg` (plugins) and `.oxpart` (partial transfers)
- [ ] GitHub: protect `main` (PR + CI required), delete merged branches, decide on untracked `plans/visuals/`

## Implementation Steps

1. Agent prepares a decision pack (`plans/reports/decision-pack-*.md`): every open item with options, the design set's default, impact and the phase it blocks — no recommendation presented as decided.
2. Owners decide; each decision lands as a small OpenSpec change that removes its marker and records who decided and when.
3. Repository admin applies branch protection and branch cleanup (outward-facing; needs explicit approval).

## Todo

- [x] Decision pack written (`plans/reports/decision-pack-261124-1824-owner-decisions.md`)
- [ ] Decision pack reviewed by owners
- [ ] D1 (UI framework) decided or bake-off scheduled (phase 11)
- [ ] D4, D5, D6, D7, D8, D9 decided
- [ ] Quiet period for FR-CO01-011 decided
- [ ] Extension names decided
- [ ] 71 open questions triaged by owner and due date
- [ ] Branch protection on `main` enabled
- [ ] Merged branches deleted

## Success Criteria

- `just spec-validate` passes after every decision change
- `grep -rc 'TO CONFIRM' openspec/` decreases with each decision; remaining items have owners and dates
