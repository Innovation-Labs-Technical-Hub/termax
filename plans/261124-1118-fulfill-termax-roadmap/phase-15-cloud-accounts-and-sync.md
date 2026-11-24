---
title: "Phase 15: Cloud Accounts and Sync"
status: todo
priority: P2
effort: "10 person-weeks"
dependencies: [14, 1]
---

# Phase 15: Cloud Accounts and Sync

## Overview

Add accounts and end-to-end encrypted sync (Pro tier) with the closed-source `oxsync` service in a separate private repo.

Owner: Backend engineer · Priority: P2 · Effort: 10 person-weeks · Depends on: phase 14, phase 1

## Requirements

- [ ] `cloud-accounts-sync` CL-01 (10 P2, 2 P3): OPAQUE login, devices, recovery
- [ ] CL-02 (12 P2, 2 P3): E2EE sync protocol, conflict handling, `termax-sync` client
- [ ] Hosting decision D6 (Fly.io + Neon default)

## Implementation Steps

1. Create the private `termax-cloud` repo (outside this repo; `.principles/planned-stacks.md` cloud rules).
2. Change `add-sync-client` in this repo (client side only).
3. Zero-knowledge tests: the server never sees plaintext or unwrapped keys.

## Todo

- [ ] termax-cloud repo created
- [ ] CL-01 implemented
- [ ] CL-02 implemented
- [ ] Sync client merged here

## Success Criteria

- Sync conflict test corpus
- Zero-knowledge audit test
- `cargo xtask spec-coverage --phase P2` for CL-01/02
