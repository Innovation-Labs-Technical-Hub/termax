---
title: "Phase 16: Team, Multiplayer and Billing"
status: todo
priority: P2
effort: "10 person-weeks"
dependencies: [15]
---

# Phase 16: Team, Multiplayer and Billing

## Overview

Add team vaults, multiplayer sessions, plans and billing, and the team UI.

Owner: Backend engineer + UI engineer · Priority: P2 · Effort: 10 person-weeks · Depends on: phase 15

## Requirements

- [ ] `team-admin-billing` CL-03 (9 P2, 1 P3), CL-04 (8), CL-05 (7 P2, 2 P3; billing provider D7), UI-11 (7)

## Implementation Steps

1. Changes per module; billing after D7.
2. Multiplayer relay end-to-end encrypted; observe/operate grants reuse the policy engine.

## Todo

- [ ] CL-03 done
- [ ] CL-04 done
- [ ] CL-05 done
- [ ] UI-11 done

## Success Criteria

- Relay E2EE tests
- Billing sandbox tests
- `cargo xtask spec-coverage --phase P2`
