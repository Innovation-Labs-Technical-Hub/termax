//! End-to-end policy scenarios across analysis, rules and the engine.

use std::sync::Arc;
use std::time::Duration;

use termax_core::Env;

use crate::*;

fn allow_all_rm() -> RuleSet {
    RuleSet::from_toml(
        r#"
        [[rule]]
        program = "rm"
        effect = "allow"
        [[rule]]
        program = "echo"
        effect = "allow"
        [[rule]]
        program = "*"
        effect = "allow"
        reason = "allow everything"
        "#,
    )
    .unwrap()
}

fn deny_reason(cmd: &str) -> String {
    let v = allow_all_rm().evaluate(cmd);
    assert_eq!(v.effect, Effect::Deny, "`{cmd}` should be denied: {v:?}");
    v.reasons.join("; ")
}

fn effect(cmd: &str) -> Effect {
    allow_all_rm().evaluate(cmd).effect
}

#[test]
fn rm_rf_root_hidden_in_command_substitution_is_denied() {
    let reason = deny_reason("echo hi && $(rm -rf /)");
    assert!(reason.contains("recursive delete of `/`"), "{reason}");
}

#[test]
fn mkfs_behind_sudo_bash_c_is_denied() {
    let reason = deny_reason("sudo bash -c 'mkfs.ext4 /dev/sda1'");
    assert!(reason.contains("mkfs.ext4"), "{reason}");
}

#[test]
fn user_allow_rule_cannot_relax_builtin_denies() {
    for cmd in [
        "rm -rf /",
        "rm -fr /*",
        "rm -r -f ~",
        "rm --recursive --force \"$HOME\"",
        "rm -rf -- //",
        "/bin/rm -Rf ${HOME}/*",
        "rm --no-preserve-root -rf /x",
        "mkfs /dev/sdb",
        "dd if=/dev/zero of=/dev/nvme0n1 bs=1M",
        "echo x > /dev/sda",
        "cat img | tee /dev/disk2",
        ":(){ :|:& };:",
        "chmod -R 777 /",
        "env -S 'rm -rf /'",
        "nohup timeout 5 sudo -u root rm -rf /",
        "busybox rm -rf /",
        r"find / -exec rm -rf / \;",
        "echo $'rm -rf /' | xargs -0 bash -c 'r\\m -rf /'",
        "eval 'rm -rf /'",
    ] {
        deny_reason(cmd);
    }
}

#[test]
fn builtin_denies_have_no_false_positives_on_safe_paths() {
    for cmd in [
        "rm -rf ./build",
        "rm -rf build/ target",
        "rm -rf /tmp/termax-cache",
        "rm -rf ~/project/node_modules",
        "rm -f /",
        "dd if=disk.img of=/dev/null",
        "dd if=/dev/sda of=backup.img",
        "echo x > /dev/null 2>&1",
        "cat /dev/sda | head -c 512 > mbr.bin",
        "chmod -R 755 ./dist",
        "chmod 777 /",
        "echo 'rm -rf /'",
    ] {
        assert_eq!(effect(cmd), Effect::Allow, "`{cmd}` should be allowed");
    }
}

#[test]
fn power_commands_ask_even_when_allowed() {
    for cmd in [
        "reboot",
        "sudo shutdown -h now",
        "systemctl poweroff",
        "halt",
    ] {
        assert_eq!(effect(cmd), Effect::Ask, "`{cmd}`");
    }
}

#[test]
fn run_time_values_fail_closed_to_ask() {
    for cmd in [
        "rm -rf $TARGET",
        "$CMD",
        "echo x > \"$DEV\"",
        "bash -c \"$S\"",
    ] {
        assert_eq!(effect(cmd), Effect::Ask, "`{cmd}`");
    }
}

#[test]
fn parse_errors_fail_closed() {
    for cmd in ["echo 'unterminated", "if then fi", "echo $(", "ls )"] {
        let v = allow_all_rm().evaluate(cmd);
        assert!(v.effect >= Effect::Ask, "`{cmd}`: {v:?}");
    }
    let v = allow_all_rm().evaluate("echo 'oops");
    assert!(v.reasons.iter().any(|r| r.contains("could not be parsed")));
}

fn engine(config: PolicyConfig) -> (PolicyEngine, ManualClock) {
    let clock = ManualClock::new(1_000);
    (
        PolicyEngine::with_clock(config, Arc::new(clock.clone())),
        clock,
    )
}

fn allow_ls() -> PolicyConfig {
    PolicyConfig {
        rules: RuleSet::from_toml("[[rule]]\nprogram = \"ls\"\neffect = \"allow\"").unwrap(),
        rate_limit: None,
    }
}

fn req<'a>(actor: &'a Actor, command: &'a str, env: Env, tags: &'a [String]) -> EvalRequest<'a> {
    EvalRequest {
        actor,
        command,
        env,
        tags,
    }
}

#[test]
fn prod_env_or_tag_downgrades_allow_to_ask() {
    let (e, _) = engine(allow_ls());
    let a = Actor::InApp;
    assert_eq!(
        e.evaluate(&req(&a, "ls", Env::Dev, &[])).effect,
        Effect::Allow
    );
    let v = e.evaluate(&req(&a, "ls", Env::Prod, &[]));
    assert_eq!(v.effect, Effect::Ask);
    assert!(v.reasons[0].contains("production"));
    let tags = vec!["web".to_string(), "PROD".to_string()];
    assert_eq!(
        e.evaluate(&req(&a, "ls", Env::None, &tags)).effect,
        Effect::Ask
    );
    // Deny stays Deny on prod.
    assert_eq!(
        e.evaluate(&req(&a, "rm -rf /", Env::Prod, &[])).effect,
        Effect::Deny
    );
}

#[test]
fn external_clients_are_observe_only_until_granted() {
    let (e, _) = engine(allow_ls());
    let ext = Actor::External {
        client_id: "mcp-1".into(),
    };
    assert_eq!(e.external_mode("mcp-1"), ExternalMode::ObserveOnly);
    let v = e.evaluate(&req(&ext, "ls", Env::Dev, &[]));
    assert_eq!(v, Verdict::new(Effect::Deny, "observe-only"));

    e.set_external_mode("mcp-1", ExternalMode::Operate);
    assert_eq!(
        e.evaluate(&req(&ext, "ls", Env::Dev, &[])).effect,
        Effect::Allow
    );
    let other = Actor::External {
        client_id: "mcp-2".into(),
    };
    assert_eq!(
        e.evaluate(&req(&other, "ls", Env::Dev, &[])).effect,
        Effect::Deny
    );

    e.set_external_mode("mcp-1", ExternalMode::ObserveOnly);
    assert_eq!(
        e.evaluate(&req(&ext, "ls", Env::Dev, &[])).effect,
        Effect::Deny
    );
}

#[test]
fn kill_switch_denies_everything_while_engaged() {
    let (e, _) = engine(allow_ls());
    let a = Actor::InApp;
    let switch = e.kill_switch();
    switch.engage();
    assert!(e.kill_switch().is_engaged());
    let v = e.evaluate(&req(&a, "ls", Env::Dev, &[]));
    assert_eq!(v, Verdict::new(Effect::Deny, "kill switch engaged"));
    switch.disengage();
    assert_eq!(
        e.evaluate(&req(&a, "ls", Env::Dev, &[])).effect,
        Effect::Allow
    );
}

#[test]
fn rate_limit_uses_injected_clock_per_actor() {
    let (e, clock) = engine(PolicyConfig {
        rate_limit: Some(RateLimit {
            max_commands: 2,
            window: Duration::from_secs(10),
        }),
        ..allow_ls()
    });
    let a = Actor::InApp;
    let b = Actor::External {
        client_id: "ops".into(),
    };
    e.set_external_mode("ops", ExternalMode::Operate);
    let run = |actor: &Actor| e.evaluate(&req(actor, "ls", Env::Dev, &[]));

    assert_eq!(run(&a).effect, Effect::Allow);
    assert_eq!(run(&a).effect, Effect::Allow);
    let v = run(&a);
    assert_eq!(v.effect, Effect::Deny);
    assert!(v.reasons[0].starts_with("rate limit exceeded"));
    // Another actor has its own bucket.
    assert_eq!(run(&b).effect, Effect::Allow);
    // Half a window refills one token.
    clock.advance(Duration::from_secs(4));
    assert_eq!(run(&a).effect, Effect::Deny);
    clock.advance(Duration::from_secs(1));
    assert_eq!(run(&a).effect, Effect::Allow);
    assert_eq!(run(&a).effect, Effect::Deny);
    // A long pause refills to capacity, not beyond.
    clock.advance(Duration::from_secs(3600));
    assert_eq!(run(&a).effect, Effect::Allow);
    assert_eq!(run(&a).effect, Effect::Allow);
    assert_eq!(run(&a).effect, Effect::Deny);
}

#[test]
fn every_decision_is_audited_in_a_valid_chain() {
    let (e, clock) = engine(allow_ls());
    let a = Actor::InApp;
    e.evaluate(&req(&a, "ls", Env::Dev, &[]));
    clock.advance(Duration::from_millis(5));
    e.evaluate(&req(&a, "rm -rf /", Env::Dev, &[]));
    e.kill_switch().engage();
    e.evaluate(&req(&a, "ls", Env::Dev, &[]));

    let log = e.audit_log();
    assert_eq!(log.len(), 3);
    assert_eq!(log.verify(), Ok(()));
    let entries = log.entries();
    assert_eq!(entries[0].timestamp_ms, 1_000);
    assert_eq!(entries[1].timestamp_ms, 1_005);
    assert_eq!(entries[1].verdict, Effect::Deny);
    assert_eq!(entries[2].reasons, vec!["kill switch engaged"]);

    let mut tampered = AuditLog::from_jsonl(&log.to_jsonl().replace("rm -rf /", "ls")).unwrap();
    assert!(tampered.verify().is_err());
    tampered = AuditLog::from_jsonl(&log.to_jsonl()).unwrap();
    assert_eq!(tampered.head_hash(), log.head_hash());
}
