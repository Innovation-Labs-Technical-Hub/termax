---
title: "Fulfill the Termax roadmap"
description: "Deliver every remaining item of the 2026-11-24 checklist: owner decisions, the P1 desktop MVP, release engineering, and the P2/P3 cloud, team, mobile, plugin and enterprise scope."
status: pending
priority: P1
effort: "≈130–150 person-weeks (rough; recalibrate after phase 3)"
tags: [roadmap, openspec, p1, p2, p3]
created: 2026-11-24
---

# Fulfill the Termax roadmap

## Overview

Turns the remaining-work checklist into ordered, verifiable phases. Scope comes from `openspec/specs` (473 functional requirements: 342 P1, 109 P2, 22 P3; 166 NFRs) and the open decisions in `openspec/config.yaml`. Each phase is delivered as one or more OpenSpec changes, each on its own branch and PR, with `just ci`, `just spec-validate` and the CI perf gate green.

Already done (not in scope): workspace and CI, `termax-core`, `-policy` engine, `-interop` (ssh_config/known_hosts), `-vault` store, `-ssh` facade, `-term` + Ghostty backend (Linux default), `tmx` basics, perf harness, OpenSpec specs, `.principles/`.

## Goals

| # | Goal | Priority |
|---|------|----------|
| 1 | Every owner decision taken and recorded; no `[TO CONFIRM]` blocks the next phase | P1 |
| 2 | Progress is measured: `cargo xtask spec-coverage` reports linked, passing scenarios | P1 |
| 3 | P1 desktop MVP: all P1 `Must` requirements covered, NFR targets met, beta shipped | P1 |
| 4 | P2 cloud sync, team, mobile and plugins delivered | P2 |
| 5 | P3 enterprise scope delivered | P3 |

## Phases

| # | Phase | Status |
|---|-------|--------|
| 1 | [Owner Decisions and Repository Governance](./phase-01-start.md) | Pending |
| 2 | [Traceability and Quality Tooling](./phase-02-traceability-and-quality-tooling.md) | Pending |
| 3 | [Session Runtime and Control Endpoint](./phase-03-session-runtime-and-control-endpoint.md) | Pending |
| 4 | [SSH Completion and Other Protocols](./phase-04-ssh-completion-and-other-protocols.md) | Pending |
| 5 | [Identity, Keys, Trust and Vault](./phase-05-identity-keys-trust-and-vault.md) | Pending |
| 6 | [SFTP Engine](./phase-06-sftp-engine.md) | Pending |
| 7 | [Import, Automation and Observability](./phase-07-import-automation-and-observability.md) | Pending |
| 8 | [Agentic: Policy, Assistant and MCP](./phase-08-agentic-policy-assistant-and-mcp.md) | Pending |
| 9 | [CLI and OS Integration](./phase-09-cli-and-os-integration.md) | Pending |
| 10 | [Ghostty on macOS and Windows](./phase-10-ghostty-on-macos-and-windows.md) | Pending |
| 11 | [Desktop UI Foundation](./phase-11-desktop-ui-foundation.md) | Pending |
| 12 | [Desktop UI Feature Screens](./phase-12-desktop-ui-feature-screens.md) | Pending |
| 13 | [Release Engineering](./phase-13-release-engineering.md) | Pending |
| 14 | [MVP Release Gate](./phase-14-mvp-release-gate.md) | Pending |
| 15 | [Cloud Accounts and Sync](./phase-15-cloud-accounts-and-sync.md) | Pending |
| 16 | [Team, Multiplayer and Billing](./phase-16-team-multiplayer-and-billing.md) | Pending |
| 17 | [Mobile Apps](./phase-17-mobile-apps.md) | Pending |
| 18 | [Plugin Runtime](./phase-18-plugin-runtime.md) | Pending |
| 19 | [Enterprise and P3](./phase-19-enterprise-and-p3.md) | Pending |

## Order and parallelism

- Start now, in parallel: **1** (owners), **2** (tooling), **10** (tracks upstream).
- After 2: **3**, then **4, 5, 6, 7, 8, 9** in parallel streams with disjoint crates.
- After D1 (phase 1): **11**, then **12** (needs the core phases it renders). **13** can start once D4 is decided.
- **14** gates the beta; **15–19** follow in P2/P3.

## Dependencies and blockers

- D1 blocks phases 11–12; D4 blocks phase 13 packaging; D5 the Mosh part of 4; D6/D7 phases 15–16; D8 observability in 7; D9 MCP defaults in 8.
- Cloud (15, 16, 19) and mobile (17) code lives in separate repos per `.principles/planned-stacks.md`.

## Success Criteria

- [ ] `cargo xtask spec-coverage --phase P1` reports 100 % of P1 `Must` scenarios covered by passing tests
- [ ] `cargo xtask perf --check` passes on reference machines for each OS
- [ ] No `[TO CONFIRM · CHỜ XÁC NHẬN]` marker blocks shipped scope
- [ ] Beta crash-free sessions ≥99.5 %; P2 and P3 coverage reported per phase

<!-- slug: fulfill-termax-roadmap -->
