---
title: "Phase 18: Plugin Runtime"
status: todo
priority: P2
effort: "6 person-weeks"
dependencies: [8, 1]
---

# Phase 18: Plugin Runtime

## Overview

Add WASM plugins with capabilities, signing and safe exposure to agents.

Owner: AI / agentic engineer · Priority: P2 · Effort: 6 person-weeks · Depends on: phase 8, phase 1

## Requirements

- [ ] `plugins` AG-04 (16 P2, 1 P3): wasmtime components, WIT world `termax:plugin/host`, capabilities, signing/revocation, Unverified handling
- [ ] Package extension naming decision (phase 1)

## Implementation Steps

1. Change `add-plugin-runtime` in a new crate `termax-plugin`.
2. Policy integration: plugin tools go through `termax-policy`; Unverified plugins never exposed to external MCP clients without opt-in.

## Todo

- [ ] termax-plugin merged
- [ ] Signing and revocation
- [ ] Sample plugin

## Success Criteria

- Capability-escape tests
- `cargo xtask spec-coverage --phase P2` for AG-04
