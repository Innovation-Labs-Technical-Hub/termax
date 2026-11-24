# termax-policy

## Purpose

`termax-policy` decides whether a shell command proposed by an AI agent may run on a remote host. The answer is `Effect::Allow`, `Effect::Ask` or `Effect::Deny`, and every decision is appended to a hash-chained audit log. The command line is parsed with tree-sitter-bash into every simple command it would run. Built-in rules that cannot be relaxed come first, then user rules, and the most restrictive result wins. The crate also sanitises command output before it reaches a model. It is pure logic: it holds no network or storage code, and the audit log lives in memory (`AuditLog::to_jsonl` and `from_jsonl` serialize it; persisting it is the caller's job).

## Public API entry points

- `PolicyEngine::new(PolicyConfig)` or `with_clock(PolicyConfig, Arc<dyn Clock>)`, then `evaluate(&EvalRequest) -> Verdict`, plus `kill_switch()`, `set_external_mode`, `external_mode` and `audit_log()`.
- `EvalRequest` (with `Actor::InApp` or `Actor::External`, the host `Env` and tags), `ExternalMode`, `KillSwitch`, `RateLimit`, `ManualClock` and `SystemClock`.
- `RuleSet::from_toml`, `Rule`, `ArgMatcher`, `Effect`, `Verdict`.
- `analyze(&str) -> Analysis` (`SimpleCommand`, `Word`, `Issue`, `MAX_COMMAND_LEN`).
- `AuditLog`, `AuditEntry`, `AuditError`, `GENESIS_HASH`, `sanitize_output`, `REDACTED`, `PolicyError`.

## Commands

- `cargo test -p termax-policy` or `just test termax-policy`.
- `cargo clippy -p termax-policy --all-targets -- -D warnings`.
- `just policy "sudo rm -rf /" prod` dry-runs a command through `tmx policy check`. That CLI exits 0 for allow, 2 for ask and 3 for deny, but the `just` recipe ignores the exit status.

## Layout

- `src/analysis.rs`: tree-sitter-bash walk, wrapper unwrapping, redirection targets and structural `Issue`s.
- `src/builtin.rs`: non-overridable built-in findings (for example recursive deletes of `/` or `~`, writes to block devices, and `dd`, `chmod` and `chown` checks).
- `src/rules.rs`: the `Effect` lattice, user `Rule`s and TOML loading.
- `src/engine.rs`: actors, external modes, kill switch, rate limit and audit recording.
- `src/audit.rs`: the SHA-256 hash chain and `verify`. `src/sanitize.rs`: escape stripping and secret redaction.
- `src/tests.rs`: end-to-end scenarios across analysis, rules and the engine. Module unit tests sit next to the code.

## Conventions

- The lattice is Deny > Ask > Allow, and the default is Ask. A new check may only make a verdict stricter. User rules can never relax a built-in finding.
- `PolicyEngine::evaluate` checks in a fixed order: kill switch, observe-only external actor, rate limit, then command rules. An Allow on a production host (`Env::Prod` or a `prod` tag) is downgraded to Ask.
- `analyze` never fails. Anything it cannot understand becomes an `Issue` of at least Ask: parse errors, input longer than `MAX_COMMAND_LEN` (64 KiB), nesting deeper than the re-parse limit, and an unavailable parser.

## Gotchas

- Wrappers are stripped before rules match. `sudo`, `doas`, `env`, `nohup`, `nice`, `timeout`, `xargs`, `exec`, `command` and similar programs are unwrapped to reach the inner command, and `bash -c`, `sh -c`, `eval` and `su -c` scripts are re-parsed. `SimpleCommand::program` is the post-wrapper program, and `wrappers` lists what was stripped. A bare `sudo -s` counts as starting a shell.
- Arguments containing run-time expansions (`$VAR`, `$(...)`) are marked `Word::dynamic`. Built-in checks raise Ask for dynamic arguments to argument-sensitive programs (`ARG_SENSITIVE` in `builtin.rs`) and for dynamic redirect targets, rather than treating them as safe.
- External MCP clients are observe-only until `set_external_mode(.., ExternalMode::Operate)` is called. Do not change that default.
- `sanitize_output` removes C0 and C1 controls (including `\r`) and escape sequences, keeping only newlines and tabs, and then redacts private keys, JWTs, AWS keys and similar secrets. Run it on anything that goes to a model.

## OpenSpec

- `agentic-ai-mcp`, module AG-03 (`FR-AG03-*`): decision order, per-sub-command AST analysis, built-in rules, host environments, user rules, dry-run, hash-chained audit, output sanitisation, and the kill switch. Implementation is partial; check each requirement's scenarios before claiming it is met.

## See also

[.principles/rust.md](../../.principles/rust.md), [.principles/security.md](../../.principles/security.md), [.principles/agentic-coding.md](../../.principles/agentic-coding.md), [openspec/AGENTS.md](../../openspec/AGENTS.md).
