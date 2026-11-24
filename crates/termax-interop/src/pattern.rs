//! OpenSSH-style wildcard matching (`*`, `?`, `!negation`).

/// Match `text` against a glob-like `pattern` where `*` matches any sequence
/// and `?` matches exactly one character (OpenSSH `match_pattern`).
pub(crate) fn wildcard_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            backtrack = Some((pi, ti));
            pi += 1;
        } else if let Some((star_p, star_t)) = backtrack {
            pi = star_p + 1;
            ti = star_t + 1;
            backtrack = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Outcome of matching a value against a list of possibly negated patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListMatch {
    /// A negated pattern matched; the list as a whole must not match.
    Negated,
    /// At least one positive pattern matched and no negated one did.
    Matched,
    NoMatch,
}

/// Match `value` against `patterns` with OpenSSH `match_pattern_list`
/// semantics: any matching `!pattern` vetoes the whole list.
pub(crate) fn match_list<'a>(
    value: &str,
    patterns: impl IntoIterator<Item = &'a str>,
    case_insensitive: bool,
) -> ListMatch {
    let value = if case_insensitive {
        value.to_lowercase()
    } else {
        value.to_owned()
    };
    let mut matched = false;
    for raw in patterns {
        let (negated, pat) = match raw.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, raw),
        };
        let pat = if case_insensitive {
            pat.to_lowercase()
        } else {
            pat.to_owned()
        };
        if wildcard_match(&value, &pat) {
            if negated {
                return ListMatch::Negated;
            }
            matched = true;
        }
    }
    if matched {
        ListMatch::Matched
    } else {
        ListMatch::NoMatch
    }
}

/// True if `pattern` contains wildcard or negation syntax.
pub(crate) fn is_wildcard(pattern: &str) -> bool {
    pattern.starts_with('!') || pattern.contains(['*', '?'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard_match("web1.example.com", "*.example.com"));
        assert!(wildcard_match("web1", "web?"));
        assert!(!wildcard_match("web10", "web?"));
        assert!(wildcard_match("abc", "*"));
        assert!(wildcard_match("", "*"));
        assert!(wildcard_match("aXbXc", "a*b*c"));
        assert!(!wildcard_match("aXbXd", "a*b*c"));
        assert!(wildcard_match("abc", "abc"));
        assert!(!wildcard_match("abcd", "abc"));
    }

    #[test]
    fn lists_with_negation() {
        assert_eq!(
            match_list("Web1", ["web*", "!web2"], true),
            ListMatch::Matched
        );
        assert_eq!(
            match_list("web2", ["web*", "!web2"], true),
            ListMatch::Negated
        );
        assert_eq!(match_list("db", ["web*"], true), ListMatch::NoMatch);
        assert_eq!(match_list("Bob", ["bob"], false), ListMatch::NoMatch);
    }
}
