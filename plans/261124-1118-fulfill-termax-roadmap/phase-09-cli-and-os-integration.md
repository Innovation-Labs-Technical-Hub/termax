---
title: "Phase 9: CLI and OS Integration"
status: todo
priority: P1
effort: "5 person-weeks"
dependencies: [3, 6]
---

# Phase 9: CLI and OS Integration

## Overview

Complete the `tmx` CLI and the OS integration the desktop app needs.

Owner: Tech lead + Platform · Priority: P1 · Effort: 5 person-weeks · Depends on: phase 3, phase 6

## Requirements

- [ ] `automation-logging-cli` PL-04 (12): `tmx ssh|sftp|scp|mcp`, attach to live sessions via the control endpoint, `--json`, shell completion
- [ ] `platform-engineering` PL-03 (11): OS integration and power (EcoQoS/QoS, URL handlers, notifications, keychain/biometrics)

## Implementation Steps

1. Change `complete-tmx-cli` (PL-04).
2. Change `add-os-integration` (PL-03), per platform behind cfg.

## Todo

- [ ] PL-04 change merged
- [ ] PL-03 change merged

## Success Criteria

- `just test termax-cli` end-to-end against the sshd fixture
- CI on all 5 platforms
- `cargo xtask spec-coverage` for PL-03/04
