# Decision pack — owner decisions for the Termax roadmap

Date: 2026-11-24 · Plan: `plans/261124-1118-fulfill-termax-roadmap` (phase 1)

Every item below needs an owner's decision. Defaults come from the design set and are **not** decisions. Record each answer as a small OpenSpec change that removes its `[TO CONFIRM · CHỜ XÁC NHẬN]` marker and names who decided and when (`openspec/AGENTS.md` → Confirmation markers).

Totals: 9 architecture decisions, 3 other product decisions, 71 BRD open questions, 3 governance actions.

## 1. Architecture decisions (ARCH-BRIEF §12)

| ID | Decision | Default / context | Options | Owner | Unblocks phase |
|---|---|---|---|---|---|
| D1 | Desktop UI framework | GPUI + gpui-kit (brief default) after a 3-week bake-off vs Tauri 2 + xterm.js; gate: ≥16 panes @60 fps, idle CPU <0.5 %, idle RSS <150 MB, IME on Wayland + Windows, VoiceOver/Narrator. | Run the bake-off (phase 11 step 1) · Commit to GPUI now · Commit to Tauri now | Product owner + Tech lead + UI lead | 11, 12 (≈130 requirements) |
| D2 | GPUI source pin | gpui-pre 0.3.10 + gpui-kit 0.8.1, or a Zed git revision. | gpui-pre + gpui-kit · Zed git rev | UI lead | 11 (only if D1 = GPUI) |
| D3 | Product name and trademark | Name is Termax (decided 2026-11-24); trademark search not done. | Commission a trademark search · Accept risk for beta | Product owner (legal) | 13, 14 (public release) |
| D4 | Publishing legal entity and Windows signing | Apple Developer ($99/yr); Windows: Azure Artifact Signing only for US/CA/EU/UK entities, a Vietnamese entity needs an OV certificate on a cloud HSM (price unverified). | Vietnamese entity + OV cert on cloud HSM · Foreign entity + Azure Artifact Signing · Microsoft Store (unverified) | Product owner + Finance | 13 |
| D5 | Mosh strategy | External `mosh-client` (GPL, separate process) + Eternal Terminal first; native roaming in P3. | Accept default · Skip Mosh in P1 | Tech lead | 4 (CO-07) |
| D6 | Cloud hosting | Fly.io (2× shared-cpu-1x 1 GB ≈ $13.40/mo) + Neon Launch; alternative Cloudflare Durable Objects. | Fly.io + Neon · Cloudflare DO · Other | Tech lead + Backend engineer | 15 |
| D7 | Billing provider | Paddle (merchant of record, VN entity) or Stripe (availability in Vietnam unverified). | Paddle · Stripe | Product owner + Finance | 16 |
| D8 | Telemetry | Off by default; opt-in crash reports. | Accept default · Opt-out telemetry | Product owner + Legal / DPO | 7 (CO-13) |
| D9 | External agents' default | Observe-only by default (already implemented in termax-policy); operate needs explicit grant + approvals. | Accept default · Allow operate on non-prod by default | Product owner + Security engineer | 8 |

## 2. Other product decisions

| ID | Decision | Context | Options | Owner | Unblocks phase |
|---|---|---|---|---|---|
| FR-CO01-011 | Quiet period before idle compaction of terminal history | Not specified in the design set. | 30 s · 60 s · 5 min · other | Tech lead | 3 |
| EXT-1 | Plugin package extension `.oxtpkg` (codename-derived) | Keep, or rename (e.g. `.tmxpkg`). | Keep · Rename | Product owner | 18 |
| EXT-2 | Partial-transfer suffix `.oxpart` (codename-derived) | Keep, or rename (e.g. `.tmxpart`). | Keep · Rename | Product owner | 6 |

## 3. Governance actions (need explicit approval; outward-facing)

| Action | Detail | Who |
|---|---|---|
| Protect `main` | Require a PR and passing `test`, `spec`, `deny`, `perf` checks. | Repository admin |
| Delete merged branches | `feat/p0-foundations`, `feat/ghostty-default`, `docs/openspec-and-principles`. | Repository admin |
| Untracked `plans/visuals/` | Not created by this work; keep, commit or remove. | Repository owner |

## 4. BRD open questions by owner

Ordered by the earliest plan phase each answer unblocks. Source: `## Open questions` in each capability spec.

### Product owner (13)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-03-04 | terminal-workspace | 3, 11, 12 | Are graphics protocols (kitty/sixel) needed before P3, and are they a factor in choosing libghostty-vt? |
| Q-06-02 | identity-keys-trust | 5 | Default agent key lifetime (unlimited / 8 h / until vault lock) and the default confirm mode for local keys? |
| Q-06-05 | identity-keys-trust | 5 | Audit firm, budget and timing (before public beta or only before GA)? |
| Q-05-03 | file-transfer | 6 | Is remote → remote transfer (host A to host B) via the client in P1 scope, or P2? |
| Q-02-02 | hosts-and-import | 7, 12 | Which columns does Termius's actual CSV export contain? Sample files are needed. |
| Q-07-01 | automation-logging-cli | 7, 9 | Should command history sync via E2EE cloud (Pro), or stay local forever? |
| Q-07-05 | automation-logging-cli | 7, 9 | Does the binary name ‘tmx’ collide (D3 trademark, package managers), and should the GUI binary accept ‘termax @host’? |
| Q-01-02 | app-shell-design-system | 11 | Is a named workspace a set of windows or only a tab group within one window; does it sync across devices in P2? |
| Q-00-02 | product-foundation | 14 | Final product name and trademark (D3)? |
| Q-00-03 | product-foundation | 14 | Publishing legal entity and Windows signing path (D4)? |
| Q-11-02 | team-admin-billing | 16, 19 | Do viewers count as paid seats, and what is the Team plan's team-vault limit? |
| Q-12-01 | mobile-apps | 17 | Is the mobile app free in local mode (ARCH-BRIEF §1) or Pro-only (§11 lists 'mobile' under Pro)? |
| Q-09-01 | plugins | 18 | Self-hosted registry or a GitHub-based index (as many extension ecosystems use), and who reviews with what SLA? |

### Security engineer (10)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-03-05 | terminal-workspace | 3, 11, 12 | Should the paste-guard risk list reuse AG-03's AST rules directly, or keep a lightweight regex list for the paste path? |
| Q-04-04 | connectivity | 3, 4 | Does russh expose the negotiated algorithms and the server's strict-kex flag before NEWKEYS so we can abort/retry, or is a patch needed? |
| Q-06-01 | identity-keys-trust | 5 | Is the Secret Key mandatory for local-only (Free) users, or only once cloud sync is enabled? |
| Q-06-04 | identity-keys-trust | 5 | Which CTAP2 crate/approach for P2 (pure-Rust HID crate vs platform APIs) after licence and maintenance checks? |
| Q-05-04 | file-transfer | 6 | SHA-256 verification via exec sha256sum is a server command: how should policy classify it (observe or ask)? |
| Q-02-01 | hosts-and-import | 7, 12 | Should `Match exec` (runs a local command) be supported as an opt-in, or always treated as unsupported? |
| Q-07-03 | automation-logging-cli | 7, 9 | Should session logs in the app folder be encrypted or plain (redacted) by default for grep-ability? |
| Q-13-03 | platform-engineering | 13 | Adopt TUF from P1, or minisign manifests first and TUF in P2? |
| Q-10-05 | cloud-accounts-sync | 15 | Which padding bucket sizes, and does the server need record kind for quota statistics? |
| Q-09-03 | plugins | 18 | Open net.fetch (host allowlist) in P2 or wait for P3, given many useful plugins (Kubernetes, cloud) need API calls? |

### Tech lead (10)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-04-01 | connectivity | 3, 4 | When the desktop app starts while a headless runtime holds live sessions: does the app adopt them over the endpoint, migrate them into its process, or wait for headless to exit? |
| Q-04-02 | connectivity | 3, 4 | Should Termax speak OpenSSH's ControlMaster mux protocol so system `ssh`/`scp` can share its connections (P2), or only its own endpoint? |
| Q-06-03 | identity-keys-trust | 5 | On Windows, should Termax offer to replace Pageant by default when it detects a PuTTY migrant? |
| Q-05-01 | file-transfer | 6 | Implement posix-rename@/copy-data in our shim or contribute to russh-sftp upstream before P1 GA? Who maintains it if upstream is slow? |
| Q-02-05 | hosts-and-import | 7, 12 | Do the 30-day Trash and 50-version history need trimming once sync (P2) adds storage cost? |
| Q-08-02 | agentic-ai-mcp | 8 | When a server caps channels (MaxSessions) or network gear lacks exec, should run_command fall back to send_keys in a dedicated agent pane (operate, ask) or return an error? |
| Q-13-04 | platform-engineering | 13 | Budget for dedicated perf hardware for nightly runs (Apple Silicon Mac, Windows arm64, Linux) as self-hosted runners? |
| Q-00-01 | product-foundation | 14 | UI framework after the bake-off: GPUI (default) or Tauri 2 (D1), and pin GPUI via gpui-pre + gpui-kit or a Zed git rev (D2)? |
| Q-00-05 | product-foundation | 14 | Mosh strategy: external mosh-client + ET first, or clean-room later (D5)? |
| Q-00-06 | product-foundation | 14 | Confirm Flutter + FRB for mobile, given analysis 05 scored native SwiftUI/Compose + UniFFI higher (4.05 vs 3.58)? |

### Platform engineer (5)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-01-04 | app-shell-design-system | 11 | Bundle the full VI/EN docs in the app (larger download) or bundle only the OXT error pages and link online for the rest? |
| Q-13-02 | platform-engineering | 13 | Should MSIX update via App Installer or the in-app updater; is the Microsoft Store a viable signing/distribution path (unverified)? |
| Q-13-05 | platform-engineering | 13 | Crash-report backend (shared with mobile, Q-12-05): self-hosted or vendor? |
| Q-12-05 | mobile-apps | 17 | Crash-report backend: self-hosted or a vendor, and in which region? |
| Q-09-04 | plugins | 18 | Signing: minisign (aligned with PL-02) or Sigstore/keyless to reduce publisher key-loss risk? |

### Product owner + Security engineer (4)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-08-01 | agentic-ai-mcp | 8 | Should users (or teams) be able to opt in to auto-allow allowlisted read-only commands on prod hosts, or keep a hard ask for every execution on prod? |
| Q-10-03 | cloud-accounts-sync | 15 | Should SSH private keys sync by default or be opt-in per key? |
| Q-10-04 | cloud-accounts-sync | 15 | Is the recovery code mandatory at sign-up or optional? |
| Q-11-05 | team-admin-billing | 16, 19 | Should account-less guests view sessions via expiring view-only links in P3? |

### Product owner (legal) (3)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-02-03 | hosts-and-import | 7, 12 | Legal: may “PuTTY”/“Termius” appear as plain-text import-source labels (nominative use), given ARCH-BRIEF bans these names in the UI? |
| Q-01-05 | app-shell-design-system | 11 | Are OFL-1.1 fonts acceptable under the licence policy (Apache/MIT/BSD/MPL-2.0 only)? |
| Q-00-07 | product-foundation | 14 | May “PuTTY”/“Termius” appear as plain-text import-source labels (nominative use)? |

### Backend engineer (2)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-08-03 | agentic-ai-mcp | 8 | Mobile approval channel (P2): oxsync E2EE relay with id-only push, or allow a summary payload in APNs/FCM? |
| Q-00-04 | product-foundation | 14 | Cloud hosting Fly.io + Neon or Cloudflare Durable Objects (D6); billing Paddle MoR or Stripe (D7)? |

### Core engineer (2)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-05-02 | file-transfer | 6 | Should we open a second SSH connection for segmented download of very large files, considering server MaxSessions/MaxStartups and customer security norms? |
| Q-09-05 | plugins | 18 | Is an Extism path needed for 'simple host function' plugins in languages without good component toolchains, or keep the component model only? |

### Mobile engineer (2)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-12-02 | mobile-apps | 17 | Minimum OS versions (proposed iOS 16+, Android 10+) and whether to ship armeabi-v7a? |
| Q-12-04 | mobile-apps | 17 | Which Android foreground-service type (time-limited dataSync, specialUse with justification, or connectedDevice) passes Google Play review for SSH sessions? |

### Terminal engineer (2)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-03-02 | terminal-workspace | 3, 11, 12 | Linux fallback renderer: the wgpu GLES backend or CPU raster (tiny-skia + softbuffer)? |
| Q-07-04 | automation-logging-cli | 7, 9 | Recording format: plain asciicast v2 or our own format with input events and OSC 133 marks (P3)? |

### UI lead (2)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-02-04 | hosts-and-import | 7, 12 | What is the maximum group nesting depth, and is the 16-pane limit for broadcast/connect in splits right? |
| Q-01-01 | app-shell-design-system | 11 | Inside a terminal on Windows/Linux, Mod = Ctrl+Shift makes Mod+T equal Mod+Shift+T and Mod+D equal Mod+Shift+D; which chords should Duplicate and Split down use there (e.g. Ctrl+Shift+Alt+…)? |

### AI engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-08-04 | agentic-ai-mcp | 8 | Should the 'OpenAI-compatible' adapter target Chat Completions, the Responses API or both, given uneven support across compatible servers (vLLM, LM Studio, OpenRouter)? |

### Design lead (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-01-03 | app-shell-design-system | 11 | High contrast: map to system colours on Windows only, or ship our own HC theme on every OS? |

### Legal (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-12-03 | mobile-apps | 17 | Apple's form still mentions ERN/CCATS; for mass-market 5D992 encryption, what must currently be filed with BIS (annual self-classification report or nothing)? |

### Legal + Finance (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-11-04 | team-admin-billing | 16, 19 | When selling to domestic Vietnamese customers through a MoR, are Vietnamese e-invoices required? |

### Legal / DPO (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-10-02 | cloud-accounts-sync | 15 | Has Decree 13/2023 been supplemented or replaced by newer personal-data legislation effective 2026, and what is the current cross-border transfer filing obligation? |

### Mobile engineer + Core engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-09-02 | plugins | 18 | Should plugins run on mobile: wasmtime on iOS without JIT (an interpreter mode) — unverified — and at what performance cost? |

### Product owner (with legal counsel) (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-08-05 | agentic-ai-mcp | 8 | What must the AI data-processing notice say, and do Team/Business tiers need DPA templates when users send output to cloud providers? |

### Product owner + Finance (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-11-01 | team-admin-billing | 16, 19 | D7: settle Paddle or Stripe; will Paddle onboard a Vietnamese entity? |

### Product owner + Platform engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-13-01 | platform-engineering | 13 | D4: publishing entity and Windows signing path (which CA, which cloud HSM, cost)? |

### Product owner + legal (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-04-05 | connectivity | 3, 4 | Mosh strategy (D5): external mosh-client, clean-room, or ET first; what is the Windows story? |

### Security engineer + Product owner (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-11-03 | team-admin-billing | 16, 19 | Audit log: E2EE under an org audit key (exported via a customer-run agent) or allow a server-readable mode for direct SIEM export? |

### Tech lead + Backend engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-10-01 | cloud-accounts-sync | 15 | D6: settle Fly.io + Neon or Cloudflare Workers + Durable Objects + R2 for oxsync? |

### Tech lead + Platform engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-04-03 | connectivity | 3, 4 | Default keepalive interval (60 s proposed) and CountMax versus the idle wakeup budget and real NAT timeouts? |

### Tech lead + Security engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-03-03 | terminal-workspace | 3, 11, 12 | Are the scrollback defaults (10k lines per tab, 512 MiB global budget, ≈1,000-line hot ring) right; is encrypted disk spill needed for very large scrollback? |

### UI lead + Designer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-03-01 | terminal-workspace | 3, 11, 12 | On Windows/Linux, Mod = Ctrl+Shift inside a terminal, so Mod+T and Mod+Shift+T (likewise Mod+D/Mod+Shift+D) both become Ctrl+Shift+T: how should the Shift variants map (e.g. Ctrl+Shift+Alt)? |

### UI lead + Security engineer (1)

| ID | Capability | Unblocks phase | Question |
|---|---|---|---|
| Q-07-02 | automation-logging-cli | 7, 9 | Should we install shell integration (OSC 7/133) on remote hosts with consent, or only provide copy-paste snippets? |

## 5. Suggested order

1. D1 (or schedule the bake-off) — it gates the most work.
2. FR-CO01-011 quiet period and questions feeding phases 3–5 (session runtime, SSH, keys/trust).
3. D4 before release engineering; D5, D8, D9 before their phases start.
4. D6, D7 and P2 questions before phase 15.

## 6. Spec vs implementation differences (found while linking tests, 2026-11-24)

For each, decide whether the code changes to match the spec or the spec changes (via an OpenSpec change). Until then the affected scenarios stay unlinked in `cargo xtask spec-coverage`.

| # | Requirement | Spec says | Code does | Owner |
|---|---|---|---|---|
| 1 | AG-03 built-in rules | Built-in deny rules carry BUILTIN-DENY ids | No rule ids on built-ins | AI engineer |
| 2 | FR-AG02-011 | Observe-only clients may run read-only allowlisted commands | Every execution by an observe-only client is denied | Product owner + Security engineer |
| 3 | AG-03 output sanitiser | OSC 8 links become "(link: url)"; redaction marker `[REDACTED:category]` | Link URLs dropped; bare `<REDACTED>` | AI engineer |
| 4 | CO-01/CO-02 error codes | Disconnects carry `TMX-<AREA>-NNN` codes and a connection log | No codes, no connection log in termax-ssh (planned in phase 3) | Tech lead |
| 5 | FR-CO11-003 | known_hosts import handles hashed entries and markers | CLI import skips hashed, `@cert-authority`, `@revoked` lines | Tech lead |
| 6 | CO-11 export | Unexpressible settings exported as `# termax:` comments | Dropped silently | Tech lead |
| 7 | FR-CO08-002 per-record encryption | Hosts, labels, tags and paths live in the encrypted payload | Host labels (and protocol, env) are plaintext index columns | Security engineer |
| 8 | FR-PL04-002 | Vault password not taken from an environment variable | `TMX_VAULT_PASSWORD` accepted | Security engineer |
| 9 | FR-PL04-009 | Documented exit-code table | `tmx` exit codes differ (policy check 0/2/3, errors 1) | Tech lead |
| 10 | FR-CO04-009 | OSC 52 reads are always refused | Reads can be answered under the Ask policy | Security engineer |
| 11 | CO-04 OSC 52 writes | Default Ask; caps 100 KB / 4 KB | Default Deny; caps 1 MiB / 4 MiB | Security engineer |
| 12 | CO-04 damage | Cursor-only change damages no rows | alacritty backend marks the cursor row dirty | Terminal engineer |
| 13 | FR-CO04-013 | Selection survives reflow | Selection cleared on reflow | Terminal engineer |
