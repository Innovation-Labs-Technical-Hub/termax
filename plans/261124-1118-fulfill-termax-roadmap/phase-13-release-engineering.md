---
title: "Phase 13: Release Engineering"
status: todo
priority: P1
effort: "5–6 person-weeks"
dependencies: [1, 2]
---

# Phase 13: Release Engineering

## Overview

Make Termax shippable: fuzzing, SBOM, supply-chain review, interop CI, signed installers and auto-update.

Owner: Platform / security engineer · Priority: P1 · Effort: 5–6 person-weeks · Depends on: phase 1, phase 2

## Requirements

- [ ] `platform-engineering` PL-01 remaining (11 P1, 1 P2): fuzz ≤10 min per PR, SBOM, cargo-vet/audit, interop vs OpenSSH/Dropbear/network gear
- [ ] PL-02 (8 P1, 3 P2): signed installers ×6 (Win/macOS/Linux × amd64/arm64), auto-update — needs D4 (legal entity, Windows signing)

## Implementation Steps

1. Change `harden-quality-gates` (PL-01).
2. Change `add-packaging-and-updates` (PL-02) after D4; release workflow on tags.

## Todo

- [ ] Fuzz budget in CI
- [ ] SBOM published per build
- [ ] cargo-vet adopted
- [ ] Interop CI job
- [ ] Signed installers for 6 targets
- [ ] Auto-update channel

## Success Criteria

- Release dry run produces 6 signed artifacts
- `cargo deny check` + `cargo vet` clean
- `cargo xtask spec-coverage` for PL-01/02
