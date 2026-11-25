---
title: "Phase 2: Traceability and Quality Tooling"
status: completed
priority: P1
effort: "1–2 person-weeks"
dependencies: []
---

# Phase 2: Traceability and Quality Tooling

## Overview

Make progress measurable: link every acceptance scenario to a test, report P1 coverage in CI, and turn on the planned lints.

Owner: Tech lead · Priority: P1 · Effort: 1–2 person-weeks · Depends on: nothing

## Requirements

- [x] `.principles/testing-and-performance.md` rule 2 (scenarios become tests)
- [x] `.principles/rust.md` planned lints (`unwrap_used`, `expect_used`, `print_stdout`)
- [x] Perf harness: memory after idle compaction (task 4.2 of `add-session-runtime`)

## Implementation Steps

1. Test-to-requirement convention: a doc line `/// Verifies: FR-XXXX-NNN acceptance N` on each test.
2. `cargo xtask spec-coverage [--phase P1] [--json]`: parse scenarios from `openspec/specs`, find `Verifies:` tags in `crates/**`, cross-check that tagged tests exist and pass in the last `cargo test --message-format json` run, print covered/total.
3. Tag the existing tests (policy, interop, vault, ssh, term, cli) with the scenarios they already verify.
4. CI: run `spec-coverage` as a report job and upload the JSON; later gate on 'no decrease'.
5. Audit `unwrap`/`expect` in library code, fix, then enable the lints workspace-wide for library crates.
6. Add `memory per tab, 10k lines (after idle compaction)` to `cargo xtask perf`.

## Todo

- [x] Tagging convention documented in `openspec/AGENTS.md`
- [x] `xtask spec-coverage` with unit tests
- [x] Existing tests tagged
- [x] CI coverage report job
- [x] Library `unwrap`/`expect` audit and lints enabled
- [x] Idle-compaction memory metric added and gated

## Success Criteria

- `cargo xtask spec-coverage --phase P1` prints a baseline number
- `just ci` and `just spec-validate` pass
- CI uploads the coverage JSON
