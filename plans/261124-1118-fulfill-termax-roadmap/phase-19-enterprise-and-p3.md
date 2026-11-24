---
title: "Phase 19: Enterprise and P3"
status: todo
priority: P3
effort: "10+ person-weeks"
dependencies: [16, 17]
---

# Phase 19: Enterprise and P3

## Overview

Deliver the P3 scope: admin console and audit export, enterprise options and the remaining P3 requirements.

Owner: Tech lead + Backend · Priority: P3 · Effort: 10+ person-weeks · Depends on: phase 16, phase 17

## Requirements

- [ ] `team-admin-billing` CL-06 (8): admin console, audit export, SSO
- [ ] 22 P3 requirements across modules (e.g. GSSAPI, Mosh/ET roaming, recording timeline, graphics evaluation)
- [ ] Self-hosted oxsync image for Enterprise

## Implementation Steps

1. List P3 requirements with `grep 'Phase: P3' openspec/specs/*/spec.md` and group them into changes.
2. Admin console in the private cloud repo.

## Todo

- [ ] CL-06 done
- [ ] P3 requirement changes merged
- [ ] Self-host image

## Success Criteria

- `cargo xtask spec-coverage --phase P3`
