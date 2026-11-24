---
title: "Phase 8: Agentic: Policy, Assistant and MCP"
status: todo
priority: P1
effort: "8–10 person-weeks"
dependencies: [3, 5]
---

# Phase 8: Agentic: Policy, Assistant and MCP

## Overview

Ship the agent features safely: persisted policy and approvals, the in-app assistant and the Termax MCP server.

Owner: AI / agentic engineer · Priority: P1 · Effort: 8–10 person-weeks · Depends on: phase 3, phase 5

## Requirements

- [ ] `agentic-ai-mcp` AG-03 (14 P1, 1 P2): persisted hash-chained audit with head anchoring, approval queue, kill switch wiring, rate limits
- [ ] AG-01 (15): MCP host, BYOK providers (Claude, OpenAI, Ollama), context chips, proposed-command cards
- [ ] AG-02 (13 P1, 2 P2): rmcp server over the control endpoint, pairing, observe-only default (D9), operate with approvals
- [ ] Security rules 6–7 in `.principles/security.md`

## Implementation Steps

1. Change `persist-policy-and-approvals` (AG-03).
2. Change `add-mcp-server` (AG-02) on top of `add-control-endpoint`.
3. Change `add-ai-assistant` (AG-01); provider keys only in the vault.

## Todo

- [ ] AG-03 change merged
- [ ] AG-02 change merged
- [ ] AG-01 change merged
- [ ] Red-team review of prompt-injection and approval bypass

## Success Criteria

- `just test termax-policy` + MCP conformance tests against an MCP client
- `tmx policy check` regression corpus
- `cargo xtask spec-coverage` for AG-01/02/03
