//! Shell command analysis: turns a command line into the list of simple
//! commands it would execute, using the tree-sitter bash grammar.

use tree_sitter::{Node, Parser};

use crate::rules::Effect;

/// Commands longer than this are not parsed; the verdict fails closed to Ask.
pub const MAX_COMMAND_LEN: usize = 64 * 1024;

/// Maximum nesting of re-parsed scripts (`bash -c`, `eval`, `su -c`, ...).
const MAX_DEPTH: usize = 8;

/// One shell word after quote removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// The word's text with quotes and escapes removed. Unresolvable
    /// expansions (`$HOME`, `$(...)`) are kept verbatim.
    pub text: String,
    /// True when the word contains a parameter, command, arithmetic or
    /// process expansion whose value is only known at run time.
    pub dynamic: bool,
}

/// A single program invocation found anywhere in the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    /// Program as written (after wrappers such as `sudo` were stripped).
    pub program: Word,
    /// Arguments after the program.
    pub args: Vec<Word>,
    /// Wrappers that were stripped to reach this command, outermost first.
    pub wrappers: Vec<String>,
    /// Source text of the command, used in verdict reasons.
    pub text: String,
}

impl SimpleCommand {
    /// Final path component of the program (`/usr/bin/rm` -> `rm`). A
    /// program computed at run time is returned verbatim.
    pub fn name(&self) -> &str {
        if self.program.dynamic {
            &self.program.text
        } else {
            basename(&self.program.text)
        }
    }

    /// Arguments joined by single spaces, as matched by argument globs.
    pub fn joined_args(&self) -> String {
        self.args
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A structural finding that constrains the verdict independently of rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Minimum effect this finding imposes.
    pub effect: Effect,
    /// Human-readable explanation.
    pub reason: String,
}

/// Result of analysing a command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    /// Every simple command that would run, including those nested in
    /// pipelines, lists, subshells, substitutions and wrappers.
    pub commands: Vec<SimpleCommand>,
    /// Targets of write redirections (`>`, `>>`, `&>`, `>|`, `<>`).
    pub write_targets: Vec<Word>,
    /// Structural findings (parse errors, fork bombs, dynamic names, ...).
    pub issues: Vec<Issue>,
}

/// Analyse a shell command line. Never fails: anything that cannot be
/// understood is reported as an [`Issue`] of at least [`Effect::Ask`].
pub fn analyze(command: &str) -> Analysis {
    let mut out = Analysis::default();
    analyze_into(command, 0, &mut out);
    out
}

fn ask(out: &mut Analysis, reason: impl Into<String>) {
    out.issues.push(Issue {
        effect: Effect::Ask,
        reason: reason.into(),
    });
}

fn analyze_into(src: &str, depth: usize, out: &mut Analysis) {
    if depth > MAX_DEPTH {
        ask(out, "command nesting too deep to analyse");
        return;
    }
    if src.len() > MAX_COMMAND_LEN {
        ask(out, "command too long to analyse");
        return;
    }
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .is_err()
    {
        ask(out, "shell parser unavailable");
        return;
    }
    let Some(tree) = parser.parse(src, None) else {
        ask(out, "shell parser failed");
        return;
    };
    let root = tree.root_node();
    if root.has_error() {
        ask(out, "command could not be parsed unambiguously");
    }
    walk(root, src, depth, out);
}

fn walk(node: Node<'_>, src: &str, depth: usize, out: &mut Analysis) {
    match node.kind() {
        "command" => command_node(node, src, depth, out),
        "file_redirect" => redirect_node(node, src, out),
        "function_definition" => function_node(node, src, out),
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, depth, out);
    }
}

fn node_text<'a>(node: Node<'_>, src: &'a str) -> &'a str {
    node.utf8_text(src.as_bytes()).unwrap_or("")
}

fn command_node(node: Node<'_>, src: &str, depth: usize, out: &mut Analysis) {
    let Some(name) = node.child_by_field_name("name") else {
        return;
    };
    let mut words = vec![word(name, src)];
    let mut cursor = node.walk();
    for arg in node.children_by_field_name("argument", &mut cursor) {
        words.push(word(arg, src));
    }
    let text = node_text(node, src).to_string();
    process_words(words, Vec::new(), &text, depth, out);
}

/// Turn a word list into commands, stripping wrappers and re-parsing
/// embedded scripts.
fn process_words(
    mut words: Vec<Word>,
    mut wrappers: Vec<String>,
    text: &str,
    depth: usize,
    out: &mut Analysis,
) {
    loop {
        let Some(program) = words.first() else {
            return;
        };
        if program.dynamic || program.text.is_empty() {
            ask(
                out,
                format!("`{text}`: command name is computed at run time"),
            );
        }
        match unwrap(&words) {
            Unwrapped::Inner(start) => {
                wrappers.push(words[0].text.clone());
                words.drain(..start);
                if words.is_empty() {
                    // A bare wrapper such as `sudo -s` starts a shell.
                    let program = Word {
                        text: wrappers.pop().unwrap_or_default(),
                        dynamic: false,
                    };
                    words.push(program);
                    break;
                }
            }
            Unwrapped::Script(script) => {
                if script.dynamic {
                    ask(out, format!("`{text}`: script is computed at run time"));
                }
                analyze_into(&script.text, depth + 1, out);
                return;
            }
            Unwrapped::None => break,
        }
    }

    let mut iter = words.into_iter();
    let Some(program) = iter.next() else {
        return;
    };
    let cmd = SimpleCommand {
        program,
        args: iter.collect(),
        wrappers,
        text: text.to_string(),
    };
    if cmd.name() == "find" {
        find_exec(&cmd, depth, out);
    }
    out.commands.push(cmd);
}

/// `find ... -exec CMD ... ;` runs CMD: analyse it as a command too.
fn find_exec(cmd: &SimpleCommand, depth: usize, out: &mut Analysis) {
    let mut i = 0;
    while i < cmd.args.len() {
        let a = cmd.args[i].text.as_str();
        if matches!(a, "-exec" | "-execdir" | "-ok" | "-okdir") {
            let inner: Vec<Word> = cmd.args[i + 1..]
                .iter()
                .take_while(|w| w.text != ";" && w.text != "+")
                .cloned()
                .collect();
            i += inner.len() + 1;
            if !inner.is_empty() {
                process_words(inner, vec!["find".into()], &cmd.text, depth, out);
            }
        }
        i += 1;
    }
}

enum Unwrapped {
    /// The wrapped command starts at this word index.
    Inner(usize),
    /// The wrapper runs this shell script.
    Script(Word),
    /// Not a wrapper.
    None,
}

/// Index of the first non-option word, skipping option clusters. Short
/// options listed in `short_val` and long options in `long_val` consume a
/// value (attached or as the next word).
fn skip_options(words: &[Word], mut i: usize, short_val: &str, long_val: &[&str]) -> usize {
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if t == "--" {
            return i + 1;
        }
        if let Some(long) = t.strip_prefix("--") {
            i += 1;
            if !long.contains('=') && long_val.contains(&long) {
                i += 1;
            }
            continue;
        }
        if t.len() < 2 || !t.starts_with('-') {
            return i;
        }
        i += 1;
        let cluster: Vec<char> = t[1..].chars().collect();
        if let Some(pos) = cluster.iter().position(|c| short_val.contains(*c))
            && pos + 1 == cluster.len()
        {
            i += 1;
        }
    }
    i
}

fn joined(words: &[Word]) -> Word {
    Word {
        text: words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        dynamic: words.iter().any(|w| w.dynamic),
    }
}

fn unwrap(words: &[Word]) -> Unwrapped {
    let name = basename(&words[0].text);
    let start = match name {
        "sudo" => skip_options(
            words,
            1,
            "ughpCUrtDRT",
            &[
                "user",
                "group",
                "host",
                "prompt",
                "close-from",
                "other-user",
                "role",
                "type",
                "chdir",
                "chroot",
                "command-timeout",
            ],
        ),
        "doas" => skip_options(words, 1, "uC", &[]),
        "nohup" | "builtin" | "busybox" | "setsid" => skip_options(words, 1, "", &[]),
        "command" => skip_options(words, 1, "", &[]),
        "exec" => skip_options(words, 1, "a", &[]),
        "nice" => skip_options(words, 1, "n", &["adjustment"]),
        "ionice" => skip_options(words, 1, "cnp", &["class", "classdata", "pid"]),
        "stdbuf" => skip_options(words, 1, "ioe", &["input", "output", "error"]),
        "time" => skip_options(words, 1, "fo", &["format", "output"]),
        "timeout" => {
            // Options, then the duration, then the command.
            skip_options(words, 1, "sk", &["signal", "kill-after"]) + 1
        }
        "xargs" => {
            let i = skip_options(
                words,
                1,
                "adEILnPs",
                &[
                    "arg-file",
                    "delimiter",
                    "max-lines",
                    "max-args",
                    "max-procs",
                    "max-chars",
                    "process-slot-var",
                ],
            );
            if i >= words.len() {
                return Unwrapped::None;
            }
            i
        }
        "env" => return unwrap_env(words),
        "eval" => return Unwrapped::Script(joined(&words[1..])),
        "su" => return unwrap_su(words),
        "bash" | "sh" | "zsh" | "dash" | "ksh" | "ash" | "mksh" => return unwrap_shell(words),
        _ => return Unwrapped::None,
    };
    if start >= words.len() && !matches!(name, "sudo" | "doas") {
        return Unwrapped::None;
    }
    Unwrapped::Inner(start)
}

fn unwrap_env(words: &[Word]) -> Unwrapped {
    let mut i = 1;
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if t == "-S" || t == "--split-string" {
            return Unwrapped::Script(joined(&words[i + 1..]));
        }
        if let Some(rest) = t
            .strip_prefix("--split-string=")
            .or_else(|| t.strip_prefix("-S"))
        {
            let mut parts = vec![Word {
                text: rest.to_string(),
                dynamic: w.dynamic,
            }];
            parts.extend_from_slice(&words[i + 1..]);
            return Unwrapped::Script(joined(&parts));
        }
        let next = skip_options(words, i, "uCP", &["unset", "chdir"]);
        if next != i {
            i = next;
            continue;
        }
        if t.contains('=') {
            i += 1;
            continue;
        }
        break;
    }
    if i >= words.len() {
        Unwrapped::None
    } else {
        Unwrapped::Inner(i)
    }
}

fn unwrap_su(words: &[Word]) -> Unwrapped {
    let mut i = 1;
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if t == "-c" || t == "--command" {
            return match words.get(i + 1) {
                Some(script) => Unwrapped::Script(script.clone()),
                None => Unwrapped::None,
            };
        }
        if let Some(script) = t.strip_prefix("--command=") {
            return Unwrapped::Script(Word {
                text: script.to_string(),
                dynamic: w.dynamic,
            });
        }
        i += 1;
    }
    Unwrapped::None
}

fn unwrap_shell(words: &[Word]) -> Unwrapped {
    let mut i = 1;
    let mut has_c = false;
    while let Some(w) = words.get(i) {
        let t = w.text.as_str();
        if matches!(t, "-o" | "+o" | "-O" | "+O") {
            i += 2;
            continue;
        }
        if t == "--" {
            i += 1;
            break;
        }
        if t.starts_with("--") {
            i += 1;
            continue;
        }
        if t.len() > 1 && (t.starts_with('-') || t.starts_with('+')) {
            has_c |= t.starts_with('-') && t.contains('c');
            i += 1;
            continue;
        }
        break;
    }
    match words.get(i) {
        Some(script) if has_c => Unwrapped::Script(script.clone()),
        _ => Unwrapped::None,
    }
}

fn redirect_node(node: Node<'_>, src: &str, out: &mut Analysis) {
    let mut cursor = node.walk();
    let is_write = node.children(&mut cursor).any(|c| {
        !c.is_named() && matches!(c.kind(), ">" | ">>" | "&>" | "&>>" | ">|" | "<>" | ">&")
    });
    if !is_write {
        return;
    }
    if let Some(dest) = node.child_by_field_name("destination") {
        // `2>&1` duplicates a descriptor; it does not name a file.
        if dest.kind() != "number" {
            out.write_targets.push(word(dest, src));
        }
    }
}

/// Detects `:(){ :|:& };:`-style fork bombs: a function that calls itself
/// through a pipeline or in the background.
fn function_node(node: Node<'_>, src: &str, out: &mut Analysis) {
    let (Some(name), Some(body)) = (
        node.child_by_field_name("name"),
        node.child_by_field_name("body"),
    ) else {
        return;
    };
    let name = node_text(name, src);
    let mut recursive = false;
    let mut spawns = false;
    scan_body(body, src, name, &mut recursive, &mut spawns);
    if recursive && spawns {
        out.issues.push(Issue {
            effect: Effect::Deny,
            reason: format!("fork bomb: function `{name}` spawns copies of itself"),
        });
    }
}

fn scan_body(node: Node<'_>, src: &str, name: &str, recursive: &mut bool, spawns: &mut bool) {
    match node.kind() {
        "pipeline" | "&" => *spawns = true,
        "command"
            if node
                .child_by_field_name("name")
                .is_some_and(|n| node_text(n, src) == name) =>
        {
            *recursive = true;
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scan_body(child, src, name, recursive, spawns);
    }
}

fn is_dynamic(node: Node<'_>) -> bool {
    if matches!(
        node.kind(),
        "simple_expansion"
            | "expansion"
            | "command_substitution"
            | "process_substitution"
            | "arithmetic_expansion"
            | "brace_expression"
    ) {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).any(is_dynamic)
}

fn word(node: Node<'_>, src: &str) -> Word {
    Word {
        text: word_text(node, src),
        dynamic: is_dynamic(node),
    }
}

fn word_text(node: Node<'_>, src: &str) -> String {
    let raw = node_text(node, src);
    match node.kind() {
        "command_name" | "concatenation" => {
            let mut cursor = node.walk();
            let parts: Vec<String> = node
                .children(&mut cursor)
                .map(|c| word_text(c, src))
                .collect();
            if parts.is_empty() {
                unescape(raw)
            } else {
                parts.concat()
            }
        }
        "word" => unescape(raw),
        "raw_string" => strip_pair(raw, "'", "'").to_string(),
        "string" => unescape_double(strip_pair(raw, "\"", "\"")),
        "ansi_c_string" => decode_ansi_c(strip_pair(raw, "$'", "'")),
        "translated_string" => unescape_double(strip_pair(raw, "$\"", "\"")),
        _ => raw.to_string(),
    }
}

fn strip_pair<'a>(s: &'a str, open: &str, close: &str) -> &'a str {
    s.strip_prefix(open)
        .and_then(|s| s.strip_suffix(close))
        .unwrap_or(s)
}

/// Removes backslash escapes from an unquoted word (`r\m` -> `rm`).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\n') | None => {}
                Some(n) => out.push(n),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Inside double quotes only `\$ \` \" \\` and line continuations are escapes.
fn unescape_double(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('$' | '`' | '"' | '\\') => out.push(chars.next().unwrap_or('\\')),
                Some('\n') => {
                    chars.next();
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decodes `$'...'` escapes so `$'\x72m'` is seen as `rm`.
fn decode_ansi_c(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            out.push('\\');
            break;
        };
        let simple = match e {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'e' | 'E' => Some('\x1b'),
            'f' => Some('\x0c'),
            'v' => Some('\x0b'),
            '\\' | '\'' | '"' | '?' => Some(e),
            _ => None,
        };
        if let Some(ch) = simple {
            out.push(ch);
            continue;
        }
        let (radix, max, mut digits) = match e {
            'x' => (16, 2, String::new()),
            '0'..='7' => (8, 2, e.to_string()),
            _ => {
                out.push('\\');
                out.push(e);
                continue;
            }
        };
        while digits.len() < max + usize::from(radix == 8) {
            match chars.peek() {
                Some(d) if d.is_digit(radix) => {
                    digits.push(*d);
                    chars.next();
                }
                _ => break,
            }
        }
        match u32::from_str_radix(&digits, radix)
            .ok()
            .and_then(char::from_u32)
        {
            Some(ch) => out.push(ch),
            None => {
                out.push('\\');
                out.push(e);
                out.push_str(digits.trim_start_matches(e));
            }
        }
    }
    out
}

/// Final path component of a program path.
pub(crate) fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(a: &Analysis) -> Vec<&str> {
        a.commands.iter().map(SimpleCommand::name).collect()
    }

    #[test]
    fn recurses_into_lists_pipelines_and_substitutions() {
        let a = analyze("echo hi && $(rm -rf /) | grep x; (cat f) || `ls`");
        assert_eq!(
            names(&a),
            vec!["echo", "$(rm -rf /)", "rm", "grep", "cat", "`ls`", "ls"]
        );
        let rm = &a.commands[2];
        assert_eq!(rm.joined_args(), "-rf /");
    }

    #[test]
    fn strips_wrappers_and_reparses_shell_scripts() {
        let a = analyze("sudo -u root bash -c 'mkfs.ext4 /dev/sda1'");
        assert_eq!(names(&a), vec!["mkfs.ext4"]);
        let a = analyze("env FOO=1 nohup timeout -s KILL 5 xargs -n1 rm -f");
        assert_eq!(names(&a), vec!["rm"]);
        assert_eq!(
            a.commands[0].wrappers,
            vec!["env", "nohup", "timeout", "xargs"]
        );
        let a = analyze("su -c 'reboot' root");
        assert_eq!(names(&a), vec!["reboot"]);
        let a = analyze("eval \"rm -rf /tmp/x\"");
        assert_eq!(names(&a), vec!["rm"]);
    }

    #[test]
    fn bash_with_script_file_is_not_unwrapped() {
        let a = analyze("bash ./deploy.sh");
        assert_eq!(names(&a), vec!["bash"]);
    }

    #[test]
    fn removes_quotes_and_escapes() {
        let a = analyze(r#"\rm -rf "$HOME" 'a b' $'\x72m' x"y"'z'"#);
        let c = &a.commands[0];
        assert_eq!(c.name(), "rm");
        let args: Vec<&str> = c.args.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(args, vec!["-rf", "$HOME", "a b", "rm", "xyz"]);
        assert!(c.args[1].dynamic);
        assert!(!c.args[2].dynamic);
    }

    #[test]
    fn captures_write_redirect_targets() {
        let a = analyze("echo x > /dev/sda 2>&1; cat < in >> out.log");
        let t: Vec<&str> = a.write_targets.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(t, vec!["/dev/sda", "out.log"]);
    }

    #[test]
    fn find_exec_is_analysed() {
        let a = analyze(r"find / -name x -exec rm -rf {} \;");
        assert_eq!(names(&a), vec!["rm", "find"]);
    }

    #[test]
    fn parse_error_and_dynamic_name_are_issues() {
        let a = analyze("echo 'unterminated");
        assert!(a.issues.iter().any(|i| i.effect == Effect::Ask));
        let a = analyze("$CMD -rf /");
        assert!(a.issues.iter().any(|i| i.reason.contains("run time")));
    }

    #[test]
    fn detects_fork_bomb() {
        let a = analyze(":(){ :|:& };:");
        assert!(a.issues.iter().any(|i| i.effect == Effect::Deny));
        let a = analyze("f() { echo hi; }; f");
        assert!(a.issues.is_empty());
    }
}
