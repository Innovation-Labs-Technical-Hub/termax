# Planned stacks principles (provisional)

Scope: stacks the design set commits to that have no code in this repository yet: desktop UI (GPUI, pending D1), MCP host and server (rmcp), WASM plugins (wasmtime), mobile (Flutter + flutter_rust_bridge, repo `termax-mobile`) and the cloud service (`oxsync` in Rust, repo `termax-cloud`). These rules record constraints already decided in the design set so the first code follows them; each section becomes its own `.principles/<stack>.md` with real enforcement when the stack lands.

## Principles

1. **One Rust core for every surface.** Desktop, CLI, mobile and agents use the same crates; UI layers render `Frame`/view-models and never reimplement SSH, terminal or vault logic. (ARCH-BRIEF §3–§4.)
2. **Every new entry point is an untrusted boundary.** MCP clients, plugins, deep links and the local control endpoint get least privilege by default and go through `termax-policy`. (ARCH-BRIEF §1, D9.)
3. **Licences decide architecture.** Apache-2.0 core; GPL code (Zed's terminal/agent crates, mosh) is never linked — only run as separate processes. The cloud service is closed-source and lives in its own repo. (ARCH-BRIEF §1.)

## Rules

### Desktop UI (GPUI, pending D1)
1. MUST NOT start production UI code before D1 (GPUI vs Tauri bake-off) is decided [TO CONFIRM · CHỜ XÁC NHẬN].
2. MUST keep the UI thread free of I/O: it pulls frames at vsync from the session runtime and sends input through `TermCore` encoders.
3. MUST meet WCAG 2.2 AA for chrome (AccessKit) and render hidden tabs at zero cost (ARCH-BRIEF §6, NFR-00-*).
4. MUST NOT copy code from GPL-licensed Zed crates; GPUI and gpui-kit are Apache-2.0.

### MCP (rmcp)
5. MUST expose Termax as an MCP server only through the local control endpoint, with per-client tokens; external clients start observe-only.
6. MUST NOT use MCP sampling or roots (deprecated in the targeted spec revisions); support 2025-11-25 and 2026-07-28 protocol versions as listed in ARCH-BRIEF §2.
7. MUST label client-reported names as unverified and keep approvals out-of-band in the Termax UI.

### Plugins (wasmtime)
8. MUST run plugins as WASM components against the WIT world `termax:plugin/host` with declared capabilities; no host filesystem or network without a granted capability.
9. MUST show unsigned or sideloaded plugins as Unverified and never expose their tools to external MCP clients without a separate opt-in (BRD-09).

### Mobile (Flutter + flutter_rust_bridge)
10. MUST call the Rust core through `termax-ffi`; Dart code holds no secrets beyond what the platform keychain returns for the current operation.
11. MUST paint the terminal from `Frame` damage only (CustomPainter) and keep 44 pt touch targets.

### Cloud (`oxsync`, closed source)
12. MUST stay zero-knowledge: the server stores only ciphertext and wrapped keys; login uses OPAQUE; no server-side decryption path exists.
13. MUST keep cloud code out of this public repository.

## Enforcement

To be added with each stack: CI jobs, lints and tests per rule. Until then, reviewers apply this file to design documents and OpenSpec changes that introduce these stacks.

## Anti-patterns

- **Prototype becomes product.** Bake-off code merged as the real UI before D1. Fix: keep spikes on branches or under `spikes/` and delete them after the decision.
- **A second implementation of core logic in Dart or TypeScript** "for speed". Fix: add an FFI or control-endpoint method instead.

## References

- ARCH-BRIEF §1–§4, §12 (D1, D9) — design set.
- `openspec/specs/app-shell-design-system`, `agentic-ai-mcp`, `plugins`, `mobile-apps`, `cloud-accounts-sync`, `team-admin-billing`.
