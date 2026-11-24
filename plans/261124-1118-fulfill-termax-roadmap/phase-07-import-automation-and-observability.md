---
title: "Phase 7: Import, Automation and Observability"
status: todo
priority: P1
effort: "6 person-weeks"
dependencies: [3, 5]
---

# Phase 7: Import, Automation and Observability

## Overview

Finish import/export, add snippets, runbooks, history and logging, and make Termax diagnosable.

Owner: Tech lead + Platform · Priority: P1 · Effort: 6 person-weeks · Depends on: phase 3, phase 5

## Requirements

- [ ] `hosts-and-import` CO-11 (11 P1, 1 P2): PuTTY registry and `~/.putty`, Termius CSV, TOML export, managed ssh_config Include, `termax://`/`ssh://` deep links, preview and rollback
- [ ] `automation-logging-cli` CO-12 (9 P1, 1 P2, 1 P3): snippets with variables, runbooks with confirmations, history, connection log with censoring
- [ ] `platform-engineering` CO-13 (7 P1, 1 P2): JSON tracing, redacted diagnostics bundle, opt-in crash reports, telemetry off by default (D8)

## Implementation Steps

1. Change `complete-import-export` (CO-11).
2. Change `add-automation-and-logging` (CO-12) — runbook rollback never auto-runs.
3. Change `add-observability` (CO-13) after D8.

## Todo

- [ ] CO-11 change merged
- [ ] CO-12 change merged
- [ ] CO-13 change merged

## Success Criteria

- `just test termax-interop` (new importer corpora)
- Diagnostics bundle passes a secret-scan test
- `cargo xtask spec-coverage` for CO-11/12/13
