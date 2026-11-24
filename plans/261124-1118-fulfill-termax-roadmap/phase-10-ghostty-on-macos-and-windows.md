---
title: "Phase 10: Ghostty on macOS and Windows"
status: todo
priority: P2
effort: "2–3 person-weeks (depends on upstream)"
dependencies: []
---

# Phase 10: Ghostty on macOS and Windows

## Overview

Make the Ghostty backend build and pass on macOS 26 SDK and Visual Studio 2026, then make it the default there too.

Owner: Terminal engineer · Priority: P2 · Effort: 2–3 person-weeks (depends on upstream) · Depends on: nothing

## Requirements

- [ ] Platform gap recorded in `openspec/config.yaml` and `plans/reports/ghostty-spike-261124-backend-comparison.md`
- [ ] NFR-00-003 memory target on all platforms

## Implementation Steps

1. Track Ghostty/Zig releases; when a pinned Ghostty builds with a Zig that supports the new SDKs, move to libghostty-vt 1.0 (`ghostty-vt` crate or a thin own binding).
2. Alternative if upstream stalls: build libghostty-vt in our own build script with the newer Zig and link it via pkg-config.
3. Flip `DEFAULT_BACKEND` per platform and drop the CI exclusion.

## Todo

- [ ] Upstream path evaluated
- [ ] Ghostty builds on macOS 26 SDK
- [ ] Ghostty builds on Windows x64/arm64
- [ ] Defaults switched, CI exclusion removed

## Success Criteria

- CI green on all 5 platforms with Ghostty included
- `just term-test` on each platform
- `just compare-backends` numbers recorded per platform
