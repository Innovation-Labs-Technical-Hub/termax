//! Line tokenisation following OpenSSH `readconf.c` / `argv_split`.

/// Split a config line into its keyword and the remainder after the
/// `Key Value` / `Key=Value` separator. Returns `None` for blank lines and
/// comments.
pub(crate) fn split_keyword(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_matches(|c: char| c.is_ascii_whitespace());
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find([' ', '\t', '=']).unwrap_or(line.len());
    let keyword = &line[..end];
    let mut rest = line[end..].trim_start_matches([' ', '\t']);
    if let Some(after_eq) = rest.strip_prefix('=') {
        rest = after_eq.trim_start_matches([' ', '\t']);
    }
    Some((keyword, rest))
}

/// Split arguments with OpenSSH quoting rules: whitespace separates
/// arguments, `"` and `'` quote, backslash escapes quotes, backslash and
/// (outside quotes) space, and a `#` at the start of an argument begins a
/// trailing comment.
pub(crate) fn split_args(input: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut args = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' || chars[i] == '\t' {
            i += 1;
            continue;
        }
        if chars[i] == '#' {
            break;
        }
        let mut arg = String::new();
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                match chars.get(i + 1) {
                    Some(&n)
                        if n == '\'' || n == '"' || n == '\\' || (quote.is_none() && n == ' ') =>
                    {
                        arg.push(n);
                        i += 2;
                    }
                    _ => {
                        arg.push(c);
                        i += 1;
                    }
                }
                continue;
            }
            match quote {
                None if c == ' ' || c == '\t' => break,
                None if c == '"' || c == '\'' => quote = Some(c),
                Some(q) if c == q => quote = None,
                _ => arg.push(c),
            }
            i += 1;
        }
        if quote.is_some() {
            return Err("unterminated quote".to_owned());
        }
        args.push(arg);
    }
    Ok(args)
}

/// Quote a single argument so that [`split_args`] yields it back unchanged.
pub(crate) fn quote_arg(arg: &str) -> String {
    let needs_quotes =
        arg.is_empty() || arg.starts_with('#') || arg.contains([' ', '\t', '"', '\'', '\\']);
    if !needs_quotes {
        return arg.to_owned();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    for c in arg.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Join arguments into one line, quoting where needed.
pub(crate) fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_separators() {
        assert_eq!(split_keyword("Port 22"), Some(("Port", "22")));
        assert_eq!(split_keyword("  Port=22  "), Some(("Port", "22")));
        assert_eq!(split_keyword("Port = 22"), Some(("Port", "22")));
        assert_eq!(split_keyword("\tUser\tbob\r"), Some(("User", "bob")));
        assert_eq!(split_keyword("   # comment"), None);
        assert_eq!(split_keyword("   "), None);
        assert_eq!(split_keyword("BatchMode"), Some(("BatchMode", "")));
    }

    #[test]
    fn argument_quoting() {
        assert_eq!(
            split_args(r#"a "b c" 'd e' f\ g h\"i # trailing"#).unwrap(),
            vec!["a", "b c", "d e", "f g", "h\"i"]
        );
        assert_eq!(split_args(r#""it's""#).unwrap(), vec!["it's"]);
        assert_eq!(split_args(r#""""#).unwrap(), vec![""]);
        assert_eq!(split_args(r"C:\path").unwrap(), vec![r"C:\path"]);
        assert_eq!(split_args("x#y").unwrap(), vec!["x#y"]);
        assert!(split_args(r#""open"#).is_err());
    }

    #[test]
    fn quoting_round_trips() {
        let args: Vec<String> = [
            "plain",
            "",
            "with space",
            "q\"uote",
            "back\\slash",
            "#hash",
            "it's",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(split_args(&join_args(&args)).unwrap(), args);
    }
}
