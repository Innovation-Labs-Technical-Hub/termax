---
title: "Phase 17: Mobile Apps"
status: todo
priority: P2
effort: "12 person-weeks"
dependencies: [14, 15]
---

# Phase 17: Mobile Apps

## Overview

Ship the mobile beta on the shared Rust core.

Owner: Flutter engineer · Priority: P2 · Effort: 12 person-weeks · Depends on: phase 14, phase 15

## Requirements

- [ ] `mobile-apps` MO-01 (13): Flutter + flutter_rust_bridge, `termax-ffi`, vault/keychain on mobile
- [ ] MO-02 (11 P2, 1 P3): terminal painted from `Frame` damage, 60 fps scroll, resume reconnect ≤2 s
- [ ] Terminal backend on mobile (memory budget; Ghostty availability on iOS/Android to verify)

## Implementation Steps

1. Create `termax-ffi` crate here; app in the separate `termax-mobile` repo.
2. Changes for MO-01 then MO-02.

## Todo

- [ ] termax-ffi merged
- [ ] MO-01 done
- [ ] MO-02 done
- [ ] Mobile beta

## Success Criteria

- FFI tests
- Device perf runs (scroll, resume)
- `cargo xtask spec-coverage --phase P2` for MO-*
