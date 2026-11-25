---
title: "Phase 3: Session Runtime and Control Endpoint"
status: todo
priority: P1
effort: "5–6 person-weeks"
dependencies: [1, 2]
---

# Phase 3: Session Runtime and Control Endpoint

## Overview

Give every client (UI, CLI, agents) one session runtime, then expose it over the local control endpoint and in headless mode.

Owner: Tech lead · Priority: P1 · Effort: 5–6 person-weeks · Depends on: phase 1, phase 2

## Requirements

- [ ] `connectivity` CO-01: FR-CO01-001…011 (10 BRD requirements + FR-CO01-011 delta)
- [ ] Interfaces IF-04-01 SessionService, IF-04-02 local control endpoint (JSON-RPC 2.0)

## Implementation Steps

1. Apply `openspec/changes/add-session-runtime` (14 tasks; new crate `termax-session`).
2. New change `add-control-endpoint`: FR-CO01-006 (UDS 0600 / named pipe with user-SID DACL, per-client tokens, pairing), FR-CO01-007 headless mode, FR-CO01-008 network-change and sleep/wake handling.
3. Archive both changes after merge.

## Todo

- [ ] `add-session-runtime` tasks 1.1–5.2 done (2026-11-25, branch `feat/session-runtime`: 11/14 verified; open: 1.1 awaits CI on macOS/Windows and `cargo deny`, 5.1 awaits the Tech lead's quiet-period decision (provisional 60 s), 5.2 awaits archive after merge)
- [ ] `add-control-endpoint` proposed, validated, implemented
- [x] Echo-under-flood metric through `termax-session` gated in CI ("echo under flood p50/p99 via termax-session", gated like the other flood metrics; first base comparison happens on the PR after this one)
- [ ] Changes archived

## Success Criteria

- `just test termax-session`, `just test-ssh`, `just perf-compare`
- `cargo xtask spec-coverage` shows all CO-01 acceptance scenarios covered
