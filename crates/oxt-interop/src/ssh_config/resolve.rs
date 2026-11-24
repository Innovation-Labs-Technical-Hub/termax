//! Effective-option resolution for a concrete alias (`ssh -G` semantics).

use std::collections::BTreeMap;

use super::{Condition, CriterionKind, SshConfig, Warning, keywords};
use crate::pattern::{ListMatch, match_list};

/// Occurrences of each keyword: one `Vec<String>` of arguments per
/// directive that contributed a value.
pub(crate) type OptionMap = BTreeMap<String, Vec<Vec<String>>>;

/// Effective options for one alias, as `ssh -G <alias>` would compute them
/// (without OpenSSH's built-in defaults beyond `hostname`, `port` and
/// `user`).
///
/// Keys are lower-case keywords. `~` and `%` tokens are expanded where
/// ssh_config(5) allows them. Command keywords (`ProxyCommand`, ...) hold
/// a single argument with the raw command string; it is never executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    /// The alias that was resolved (`%n`).
    pub alias: String,
    options: OptionMap,
    /// Problems found during expansion (unsupported tokens, ...).
    pub warnings: Vec<Warning>,
}

impl ResolvedConfig {
    /// First argument of the first value of `keyword` (case-insensitive).
    pub fn get(&self, keyword: &str) -> Option<&str> {
        self.args(keyword)
            .and_then(|a| a.first())
            .map(String::as_str)
    }

    /// Arguments of the first value of `keyword`.
    pub fn args(&self, keyword: &str) -> Option<&[String]> {
        self.all(keyword).first().map(Vec::as_slice)
    }

    /// Every value of `keyword` in the order obtained; several entries only
    /// for multi-valued keywords such as `IdentityFile`.
    pub fn all(&self, keyword: &str) -> &[Vec<String>] {
        self.options
            .get(&keyword.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Effective host name to connect to (`HostName`, else the alias).
    pub fn hostname(&self) -> &str {
        self.get("hostname").unwrap_or(&self.alias)
    }

    /// Effective port; `None` if the configured value is not a valid port.
    pub fn port(&self) -> Option<u16> {
        self.get("port").and_then(|p| p.parse().ok())
    }

    /// Effective remote user name.
    pub fn user(&self) -> &str {
        self.get("user").unwrap_or_default()
    }

    /// All resolved options, sorted by keyword.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[Vec<String>])> {
        self.options.iter().map(|(k, v)| (k.as_str(), v.as_slice()))
    }
}

impl SshConfig {
    /// Effective options for `alias`, applying every matching block in file
    /// order with first-value-wins semantics, then filling `hostname`,
    /// `port` and `user` defaults and expanding `~`/`%` tokens.
    pub fn resolve(&self, alias: &str) -> ResolvedConfig {
        let mut options = self.collect(alias);
        let mut warnings = Vec::new();
        let hostname = expand_hostname(&options, alias, &mut warnings);
        options.insert("hostname".into(), vec![vec![hostname]]);
        options
            .entry("port".into())
            .or_insert_with(|| vec![vec!["22".into()]]);
        options
            .entry("user".into())
            .or_insert_with(|| vec![vec![self.options.local_user.clone()]]);

        let ctx = TokenContext {
            hostname: first(&options, "hostname").unwrap_or(alias).to_owned(),
            alias: alias.to_owned(),
            port: first(&options, "port").unwrap_or("22").to_owned(),
            user: first(&options, "user").unwrap_or_default().to_owned(),
            local_user: self.options.local_user.clone(),
            home: self.options.home_dir.to_string_lossy().into_owned(),
        };
        for (key, values) in options.iter_mut() {
            let Some((tilde, tokens)) = keywords::expansion(key) else {
                continue;
            };
            for arg in values.iter_mut().flatten() {
                if tilde {
                    *arg = expand_tilde(arg, &ctx.home);
                }
                if tokens {
                    *arg = expand_tokens(arg, &ctx, key, &mut warnings);
                }
            }
        }
        ResolvedConfig {
            alias: alias.to_owned(),
            options,
            warnings,
        }
    }

    /// Raw (unexpanded, no defaults) options obtained for `alias`.
    pub(crate) fn collect(&self, alias: &str) -> OptionMap {
        let mut options = OptionMap::new();
        let mut memo: Vec<Option<bool>> = vec![None; self.conditions.len()];
        for block in &self.blocks {
            let mut active = true;
            for &c in &block.conditions {
                let result = match memo[c] {
                    Some(r) => r,
                    None => {
                        let r = self.eval(c, alias, &options);
                        memo[c] = Some(r);
                        r
                    }
                };
                if !result {
                    active = false;
                    break;
                }
            }
            if !active {
                continue;
            }
            for d in &block.directives {
                if keywords::is_multi_valued(&d.keyword) {
                    options
                        .entry(d.keyword.clone())
                        .or_default()
                        .push(d.args.clone());
                } else {
                    options
                        .entry(d.keyword.clone())
                        .or_insert_with(|| vec![d.args.clone()]);
                }
            }
        }
        options
    }

    fn eval(&self, condition: usize, alias: &str, options: &OptionMap) -> bool {
        match &self.conditions[condition] {
            Condition::Host { patterns, .. } => {
                match_list(alias, patterns.iter().map(String::as_str), true) == ListMatch::Matched
            }
            Condition::Match(None) => false,
            Condition::Match(Some(criteria)) => {
                let host = match first(options, "hostname") {
                    Some(h) => {
                        expand_percent(h, |t| match t {
                            'h' => Some(alias.to_owned()),
                            _ => None,
                        })
                        .0
                    }
                    None => alias.to_owned(),
                };
                let user = first(options, "user").unwrap_or(&self.options.local_user);
                criteria.iter().all(|c| {
                    let list = |s: &str| s.split(',').map(str::to_owned).collect::<Vec<_>>();
                    let hit = |value: &str, pats: &str, ci: bool| {
                        let pats = list(pats);
                        match_list(value, pats.iter().map(String::as_str), ci) == ListMatch::Matched
                    };
                    let matched = match &c.kind {
                        CriterionKind::All => true,
                        CriterionKind::Host(p) => hit(&host, p, true),
                        CriterionKind::OriginalHost(p) => hit(alias, p, true),
                        CriterionKind::User(p) => hit(user, p, false),
                        CriterionKind::LocalUser(p) => hit(&self.options.local_user, p, false),
                    };
                    matched != c.negated
                })
            }
        }
    }
}

pub(crate) fn first<'a>(options: &'a OptionMap, key: &str) -> Option<&'a str> {
    options
        .get(key)
        .and_then(|v| v.first())
        .and_then(|a| a.first())
        .map(String::as_str)
}

/// `HostName` accepts only `%h` (the alias) and `%%`.
pub(crate) fn expand_hostname(
    options: &OptionMap,
    alias: &str,
    warnings: &mut Vec<Warning>,
) -> String {
    let Some(raw) = first(options, "hostname") else {
        return alias.to_owned();
    };
    let (value, unknown) = expand_percent(raw, |t| (t == 'h').then(|| alias.to_owned()));
    for t in unknown {
        warnings.push(Warning::new(
            None,
            None,
            format!("HostName: token `%{t}` is not supported; left unexpanded"),
        ));
    }
    value
}

struct TokenContext {
    hostname: String,
    alias: String,
    port: String,
    user: String,
    local_user: String,
    home: String,
}

fn expand_tokens(arg: &str, ctx: &TokenContext, key: &str, warnings: &mut Vec<Warning>) -> String {
    let (value, unknown) = expand_percent(arg, |t| {
        Some(match t {
            'h' => ctx.hostname.clone(),
            'n' => ctx.alias.clone(),
            'p' => ctx.port.clone(),
            'r' => ctx.user.clone(),
            'u' => ctx.local_user.clone(),
            'd' => ctx.home.clone(),
            _ => return None,
        })
    });
    for t in unknown {
        warnings.push(Warning::new(
            None,
            None,
            format!("{key}: token `%{t}` is not supported; left unexpanded"),
        ));
    }
    value
}

/// Replace `%x` tokens using `lookup`; `%%` is a literal percent sign.
/// Unknown tokens are left verbatim and reported in the second element.
fn expand_percent(input: &str, lookup: impl Fn(char) -> Option<String>) -> (String, Vec<char>) {
    let mut out = String::with_capacity(input.len());
    let mut unknown = Vec::new();
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(t) => match lookup(t) {
                Some(v) => out.push_str(&v),
                None => {
                    out.push('%');
                    out.push(t);
                    unknown.push(t);
                }
            },
            None => out.push('%'),
        }
    }
    (out, unknown)
}

fn expand_tilde(arg: &str, home: &str) -> String {
    if arg == "~" {
        home.to_owned()
    } else if let Some(rest) = arg.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else {
        arg.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_expansion() {
        let (v, unknown) = expand_percent("%h-%%-%C-%", |t| (t == 'h').then(|| "x".into()));
        assert_eq!(v, "x-%-%C-%");
        assert_eq!(unknown, vec!['C']);
    }

    #[test]
    fn tilde_expansion() {
        assert_eq!(expand_tilde("~/.ssh/id", "/home/a/"), "/home/a/.ssh/id");
        assert_eq!(expand_tilde("~", "/home/a"), "/home/a");
        assert_eq!(expand_tilde("~bob/id", "/home/a"), "~bob/id");
        assert_eq!(expand_tilde("/abs", "/home/a"), "/abs");
    }
}
