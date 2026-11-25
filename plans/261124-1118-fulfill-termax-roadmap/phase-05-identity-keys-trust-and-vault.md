---
title: "Phase 5: Identity, Keys, Trust and Vault"
status: todo
priority: P1
effort: "7–8 person-weeks"
dependencies: [1, 2]
---

# Phase 5: Identity, Keys, Trust and Vault

## Overview

Complete credential handling: keychain unlock, identities and keys, the SSH agent, known-hosts trust features and the threat model.

Owner: Platform / security engineer · Priority: P1 · Effort: 7–8 person-weeks · Depends on: phase 1, phase 2

## Requirements

- [ ] `identity-keys-trust` CO-08 vault (11): OS keychain (`keyring`) behind a feature, identities/keys tables + migrations
- [ ] CO-09 keys and agent (11 P1, 1 P2): Ed25519/ECDSA/RSA generation, PPK v2/v3, OpenSSH, PEM import, deploy to host, agent client/server, FIDO2 (phase 2 item)
- [ ] CO-10 known hosts (11): `@cert-authority`, UpdateHostKeys with consent, key history, PuTTY host-key import
- [ ] CO-14 security architecture (11 P1, 1 P2): threat model, fuzzing of parsers, secret-handling tests

## Implementation Steps

1. Change `add-keychain-and-identities` (CO-08) with a vault migration and backup step.
2. Change `add-keys-and-agent` (CO-09).
3. Change `complete-host-trust` (CO-10).
4. Write the threat model (STRIDE per container) under `docs/security/`; add cargo-fuzz targets for ssh_config, known_hosts, PPK and the policy parser.

## Todo

- [ ] CO-08 change merged
- [ ] CO-09 change merged
- [ ] CO-10 change merged
- [ ] Threat model reviewed
- [ ] Fuzz targets in CI (≤10 min per PR, phase 13 wires the budget)

## Success Criteria

- `just test termax-vault`, `just test termax-ssh`
- Plaintext-scan tests extended to identities/keys tables
- `cargo xtask spec-coverage` for CO-08/09/10/14
