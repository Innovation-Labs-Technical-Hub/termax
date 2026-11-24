---
title: "Phase 6: SFTP Engine"
status: todo
priority: P1
effort: "4–5 person-weeks"
dependencies: [3]
---

# Phase 6: SFTP Engine

## Overview

Build the SFTP/SCP engine with pipelined, resumable transfers that keep up with OpenSSH.

Owner: Tech lead · Priority: P1 · Effort: 4–5 person-weeks · Depends on: phase 3

## Requirements

- [ ] `file-transfer` CO-06 (12): russh-sftp 4, pipelined reads/writes, resume, parallel transfers, OpenSSH extensions, SCP over exec
- [ ] NFR: SFTP ≥80 % of OpenSSH sftp at 100 ms RTT (ARCH-BRIEF §6)
- [ ] Partial-file suffix (`.oxpart`, naming decision in phase 1)

## Implementation Steps

1. Change `add-sftp-engine` in a new crate `termax-sftp`.
2. Add SFTP throughput (with netem delay where available) to `cargo xtask perf`, compared with the OpenSSH client.

## Todo

- [ ] `termax-sftp` crate and change merged
- [ ] Perf metric vs OpenSSH sftp added
- [ ] Resume and partial-file naming implemented per decision

## Success Criteria

- `just test termax-sftp` against the sshd fixture
- Perf ratio ≥0.8 of OpenSSH
- `cargo xtask spec-coverage` for CO-06
