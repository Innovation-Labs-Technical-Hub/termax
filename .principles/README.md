# Termax engineering principles

Short, checkable rules for this repository. Each file follows the same shape: Scope, Principles (with reasons), Rules (MUST/SHOULD), Enforcement, Anti-patterns, References. When a rule and the code disagree, fix one of them in the same PR.

| Area | File | Owner (role) | Enforced by |
|---|---|---|---|
| Rust crates and workspace | [rust.md](rust.md) | Tech lead | CI `test` (fmt, clippy `-D warnings`, `unsafe_code = forbid`), `just msrv`, review |
| Security (secrets, crypto, agents, supply chain) | [security.md](security.md) | Platform / security engineer | CI `deny` (cargo-deny), vault/ssh/policy test suites, review |
| Testing and performance | [testing-and-performance.md](testing-and-performance.md) | Terminal/render engineer | CI `test` on 5 platforms, CI `perf` regression gate |
| Agentic coding methodology | [agentic-coding.md](agentic-coding.md) | Tech lead | CI `spec` (`openspec validate --all --strict`), PR review |
| Planned stacks (GPUI, MCP, wasmtime, Flutter, cloud) | [planned-stacks.md](planned-stacks.md) | Tech lead + stack owners | Review of designs and OpenSpec changes until the stacks land |

Owners are roles from the team plan in ARCH-BRIEF §10 (design set); replace them with names when the team is staffed.

Related: root [`AGENTS.md`](../AGENTS.md) (golden rules and commands), [`openspec/AGENTS.md`](../openspec/AGENTS.md) (spec workflow), crate `AGENTS.md` files under `crates/`.

## Changing a principle

Open a PR that edits the rule and its Enforcement row together. A rule without enforcement must say "Review" and, if possible, the planned check.
