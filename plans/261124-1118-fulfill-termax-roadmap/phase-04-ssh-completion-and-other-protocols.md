---
title: "Phase 4: SSH Completion and Other Protocols"
status: todo
priority: P1
effort: "8–10 person-weeks"
dependencies: [3]
---

# Phase 4: SSH Completion and Other Protocols

## Overview

Finish the SSH transport and channel features of the MVP and add Telnet, serial, raw, local shell and Mosh/ET sessions.

Owner: Tech lead · Priority: P1 · Effort: 8–10 person-weeks · Depends on: phase 3

## Requirements

- [ ] `connectivity` CO-02 (14 P1, 1 P3): certificates, FIDO via agent, SOCKS/HTTP/ProxyCommand proxies, quirks table, password/keyboard-interactive end-to-end
- [ ] CO-03 (11 P1, 1 P2): `-R`, `-D` SOCKS, streamlocal, agent forwarding with confirmation, BDP window growth, special commands incl. BREAK
- [ ] CO-07 (6 P1, 2 P3): Telnet (RFC 854), serial (`serialport`), raw, local shell (portable-pty/ConPTY), Mosh/ET as external processes (decision D5)
- [ ] UI-06 data model for forwarding rules (screen itself in phase 12)

## Implementation Steps

1. Change `complete-ssh-transport` (CO-02): add a PAM-enabled sshd container to the fixture for password/kbd-interactive tests; interop matrix vs OpenSSH 9.6/10.x and Dropbear.
2. Change `add-remote-and-dynamic-forwarding` (CO-03): `-R`, `-D`, streamlocal, agent forwarding confirm-on-use.
3. BREAK: upstream russh patch or a facade-local fork; keep `send_break` returning Unsupported until then.
4. Change `add-other-protocols` (CO-07) after D5.

## Todo

- [ ] CO-02 change merged
- [ ] CO-03 change merged
- [ ] BREAK resolved or documented upstream
- [ ] CO-07 change merged (Mosh/ET per D5)
- [ ] Interop matrix report updated

## Success Criteria

- `just test-ssh` (OpenSSH + Dropbear + PAM container)
- `cargo xtask spec-coverage` for CO-02/03/07
- Perf: forward throughput and new-channel latency unchanged or better
