//! Sanitisation of command output before it is returned to a model.

use std::sync::LazyLock;

use regex::Regex;

/// Replacement text for redacted secrets.
pub const REDACTED: &str = "<REDACTED>";

/// (pattern, replacement) pairs applied in order.
static SECRET_PATTERNS: LazyLock<Vec<(Regex, String)>> = LazyLock::new(|| {
    let r = |p: &str| Regex::new(p).unwrap_or_else(|e| panic!("invalid secret pattern {p}: {e}"));
    vec![
        // PEM private keys, including a block truncated before its END line.
        (
            r(
                r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----(?:.*?-----END [A-Z0-9 ]*PRIVATE KEY-----|.*\z)",
            ),
            REDACTED.to_string(),
        ),
        // JSON Web Tokens.
        (
            r(r"\beyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*"),
            REDACTED.to_string(),
        ),
        // AWS access key ids.
        (
            r(r"\b(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}\b"),
            REDACTED.to_string(),
        ),
        // HTTP bearer credentials.
        (
            r(r"(?i)\b(bearer\s+)[A-Za-z0-9._~+/=-]+"),
            format!("${{1}}{REDACTED}"),
        ),
        // key=value / key: value pairs with secret-looking keys.
        (
            r(concat!(
                r#"(?i)\b([a-z0-9_.-]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credentials?)[a-z0-9_.-]*"?)"#,
                r#"(\s*[=:]\s*)("[^"\n]*"|'[^'\n]*'|[^\s"'&;,]+)"#,
            )),
            format!("${{1}}${{2}}{REDACTED}"),
        ),
    ]
});

/// Strip terminal escape sequences and control characters, then redact
/// likely secrets. Newlines and tabs are kept; everything else in C0/C1
/// (including `\r`) and DEL is removed.
pub fn sanitize_output(input: &str) -> String {
    let mut text = strip_controls(input);
    for (re, replacement) in SECRET_PATTERNS.iter() {
        if re.is_match(&text) {
            text = re.replace_all(&text, replacement.as_str()).into_owned();
        }
    }
    text
}

#[derive(Clone, Copy)]
enum State {
    Text,
    /// After ESC.
    Escape,
    /// Inside a CSI sequence (`ESC [` or U+009B).
    Csi,
    /// Inside an OSC/DCS/SOS/PM/APC string, terminated by BEL or ST.
    String,
    /// After ESC inside a string (possible ST `ESC \`).
    StringEscape,
    /// Inside an `ESC <intermediate>... <final>` sequence.
    Intermediate,
}

fn strip_controls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut state = State::Text;
    for c in input.chars() {
        state = match state {
            State::Text => match c {
                '\x1b' => State::Escape,
                '\u{9b}' => State::Csi,
                '\u{90}' | '\u{98}' | '\u{9d}' | '\u{9e}' | '\u{9f}' => State::String,
                '\n' | '\t' => {
                    out.push(c);
                    State::Text
                }
                c if c.is_control() => State::Text,
                c => {
                    out.push(c);
                    State::Text
                }
            },
            State::Escape => match c {
                '[' => State::Csi,
                ']' | 'P' | 'X' | '^' | '_' => State::String,
                '\x20'..='\x2f' => State::Intermediate,
                _ => State::Text,
            },
            State::Csi => match c {
                '\x40'..='\x7e' => State::Text,
                _ => State::Csi,
            },
            State::String => match c {
                '\x07' | '\u{9c}' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
            State::StringEscape => match c {
                '\\' => State::Text,
                '\x1b' => State::StringEscape,
                _ => State::String,
            },
            State::Intermediate => match c {
                '\x20'..='\x2f' => State::Intermediate,
                _ => State::Text,
            },
        };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_osc_and_controls() {
        let s = "\x1b[1;31mred\x1b[0m \x1b]0;title\x07ok \x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\ \x1b(Bx\r\n\tend\x00\x08\x7f\u{9b}2J";
        assert_eq!(sanitize_output(s), "red ok link x\n\tend");
    }

    #[test]
    fn keeps_plain_text_and_unicode() {
        let s = "Xin chào\nline 2\tcol";
        assert_eq!(sanitize_output(s), s);
    }

    #[test]
    fn redacts_private_keys() {
        let s = "before\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1r\n-----END OPENSSH PRIVATE KEY-----\nafter";
        assert_eq!(sanitize_output(s), "before\n<REDACTED>\nafter");
        let truncated = "x\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nmore";
        assert_eq!(sanitize_output(truncated), "x\n<REDACTED>");
        let public = "-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----";
        assert_eq!(sanitize_output(public), public);
    }

    #[test]
    fn redacts_aws_keys_and_jwts() {
        assert_eq!(
            sanitize_output("id AKIAIOSFODNN7EXAMPLE and ASIAABCDEFGHIJKLMNOP."),
            "id <REDACTED> and <REDACTED>."
        );
        let jwt =
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(sanitize_output(&format!("t: {jwt} ok")), "t: <REDACTED> ok");
        assert_eq!(
            sanitize_output("Authorization: Bearer abc.def-123"),
            "Authorization: Bearer <REDACTED>"
        );
    }

    #[test]
    fn redacts_key_value_secrets() {
        assert_eq!(
            sanitize_output("DB_PASSWORD=hunter2 user=bob"),
            "DB_PASSWORD=<REDACTED> user=bob"
        );
        assert_eq!(
            sanitize_output("https://x/api?token=abc123&page=2"),
            "https://x/api?token=<REDACTED>&page=2"
        );
        assert_eq!(
            sanitize_output(r#"{"api_key": "s3cr3t", "name": "n"}"#),
            r#"{"api_key": <REDACTED>, "name": "n"}"#
        );
        assert_eq!(sanitize_output("secret: 'a b c'"), "secret: <REDACTED>");
        assert_eq!(sanitize_output("status=ok count: 3"), "status=ok count: 3");
    }
}
