//! OpenSSH client configuration (`ssh_config(5)`) import, resolution and
//! export.
//!
//! Parsing keeps the file's structure (blocks guarded by `Host` / `Match`
//! conditions, with `Include`d files inlined under the condition that was
//! active at the `Include` line) so that [`SshConfig::resolve`] can evaluate
//! it for a concrete alias exactly like `ssh -G` would: blocks are applied
//! in order and the first obtained value for each keyword wins, except for
//! the multi-valued keywords (`IdentityFile`, `CertificateFile`,
//! `LocalForward`, `RemoteForward`, `DynamicForward`, `SendEnv`) which
//! accumulate.

mod convert;
mod keywords;
mod lexer;
mod resolve;

use std::fs;
use std::path::{Path, PathBuf};

pub use convert::{
    ImportReport, MULTI_VALUE_SEPARATOR, SkippedPattern, decode_setting, encode_setting,
    to_ssh_config,
};
pub use resolve::ResolvedConfig;

use crate::error::InteropError;
pub use crate::warning::Warning;

/// Maximum nesting of `Include` directives (OpenSSH `READCONF_MAX_DEPTH`).
pub const MAX_INCLUDE_DEPTH: usize = 16;

/// Environment used while parsing and resolving; injectable so callers and
/// tests never depend on the real home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOptions {
    /// Expansion of `~` and the `%d` token.
    pub home_dir: PathBuf,
    /// Base directory for relative `Include` paths (`~/.ssh` for a user
    /// config, `/etc/ssh` for the system config).
    pub ssh_dir: PathBuf,
    /// Local user name: the `%u` token and the default remote user.
    pub local_user: String,
}

impl ParseOptions {
    /// Options for `home_dir`, with `ssh_dir` set to `home_dir/.ssh`.
    pub fn new(home_dir: impl Into<PathBuf>, local_user: impl Into<String>) -> Self {
        let home_dir = home_dir.into();
        Self {
            ssh_dir: home_dir.join(".ssh"),
            home_dir,
            local_user: local_user.into(),
        }
    }

    /// Options derived from `HOME`/`USERPROFILE` and `USER`/`LOGNAME`/
    /// `USERNAME`. Returns `None` when no home directory is set.
    pub fn from_env() -> Option<Self> {
        let var = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
        };
        let home = var(&["HOME", "USERPROFILE"])?;
        let user = var(&["USER", "LOGNAME", "USERNAME"]).unwrap_or_default();
        Some(Self::new(home, user))
    }
}

/// A parsed ssh_config, ready for [`resolve`](Self::resolve) and
/// [`import`](Self::import).
#[derive(Debug, Clone)]
pub struct SshConfig {
    pub(crate) options: ParseOptions,
    pub(crate) conditions: Vec<Condition>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) warnings: Vec<Warning>,
}

#[derive(Debug, Clone)]
pub(crate) struct Directive {
    /// Lower-case keyword.
    pub(crate) keyword: String,
    pub(crate) args: Vec<String>,
}

/// Directives guarded by a conjunction of conditions (indices into
/// `SshConfig::conditions`).
#[derive(Debug, Clone)]
pub(crate) struct Block {
    pub(crate) conditions: Vec<usize>,
    pub(crate) directives: Vec<Directive>,
}

#[derive(Debug, Clone)]
pub(crate) enum Condition {
    Host {
        patterns: Vec<String>,
        file: Option<PathBuf>,
        line: usize,
    },
    /// `None` when the `Match` line used an unsupported or invalid criterion
    /// and can therefore never match.
    Match(Option<Vec<Criterion>>),
}

#[derive(Debug, Clone)]
pub(crate) struct Criterion {
    pub(crate) negated: bool,
    pub(crate) kind: CriterionKind,
}

#[derive(Debug, Clone)]
pub(crate) enum CriterionKind {
    All,
    Host(String),
    OriginalHost(String),
    User(String),
    LocalUser(String),
}

impl SshConfig {
    /// Parse ssh_config text. Relative `Include` paths are resolved against
    /// `options.ssh_dir` on the filesystem.
    pub fn parse_str(input: &str, options: &ParseOptions) -> Self {
        let mut parser = Parser::new(options);
        parser.parse_text(input, None, &[], 0);
        parser.finish()
    }

    /// Parse the config file at `path` (and the files it includes).
    pub fn parse_file(path: &Path, options: &ParseOptions) -> Result<Self, InteropError> {
        let text = fs::read_to_string(path).map_err(|source| InteropError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut parser = Parser::new(options);
        parser
            .stack
            .push(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        parser.parse_text(&text, Some(path), &[], 0);
        Ok(parser.finish())
    }

    /// Warnings collected while parsing (unknown keywords, unsupported
    /// `Match` criteria, broken includes, ...).
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// The environment this config was parsed with.
    pub fn options(&self) -> &ParseOptions {
        &self.options
    }
}

struct Parser<'o> {
    options: &'o ParseOptions,
    conditions: Vec<Condition>,
    blocks: Vec<Block>,
    warnings: Vec<Warning>,
    /// Canonical paths of the files currently being parsed (cycle guard).
    stack: Vec<PathBuf>,
}

impl<'o> Parser<'o> {
    fn new(options: &'o ParseOptions) -> Self {
        Self {
            options,
            conditions: Vec::new(),
            blocks: Vec::new(),
            warnings: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn finish(self) -> SshConfig {
        SshConfig {
            options: self.options.clone(),
            conditions: self.conditions,
            blocks: self
                .blocks
                .into_iter()
                .filter(|b| !b.directives.is_empty())
                .collect(),
            warnings: self.warnings,
        }
    }

    fn warn(&mut self, file: Option<&Path>, line: Option<usize>, message: impl Into<String>) {
        self.warnings.push(Warning::new(file, line, message));
    }

    fn open_block(&mut self, conditions: Vec<usize>) {
        self.blocks.push(Block {
            conditions,
            directives: Vec::new(),
        });
    }

    /// Parse `text` whose directives are additionally guarded by `parent`
    /// (the conditions active at the `Include` line that pulled it in).
    fn parse_text(&mut self, text: &str, file: Option<&Path>, parent: &[usize], depth: usize) {
        self.open_block(parent.to_vec());
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let Some((keyword, rest)) = lexer::split_keyword(raw) else {
                continue;
            };
            let lower = keyword.to_ascii_lowercase();
            if keywords::canonical(&lower).is_none() {
                self.warn(
                    file,
                    Some(line_no),
                    format!("unsupported or unknown keyword `{keyword}` ignored"),
                );
                continue;
            }
            let args = if keywords::is_raw_line(&lower) {
                let value = rest.trim_start_matches([' ', '\t', '=']);
                if value.is_empty() {
                    Vec::new()
                } else {
                    vec![value.to_owned()]
                }
            } else {
                match lexer::split_args(rest) {
                    Ok(args) => args,
                    Err(e) => {
                        self.warn(
                            file,
                            Some(line_no),
                            format!("`{keyword}`: {e}; line ignored"),
                        );
                        continue;
                    }
                }
            };
            if args.is_empty() {
                self.warn(
                    file,
                    Some(line_no),
                    format!("`{keyword}` has no argument; line ignored"),
                );
                continue;
            }
            match lower.as_str() {
                "host" => {
                    self.conditions.push(Condition::Host {
                        patterns: args,
                        file: file.map(Path::to_path_buf),
                        line: line_no,
                    });
                    let mut conds = parent.to_vec();
                    conds.push(self.conditions.len() - 1);
                    self.open_block(conds);
                }
                "match" => {
                    let criteria = self.parse_match(&args, file, line_no);
                    self.conditions.push(Condition::Match(criteria));
                    let mut conds = parent.to_vec();
                    conds.push(self.conditions.len() - 1);
                    self.open_block(conds);
                }
                "include" => {
                    let current = self
                        .blocks
                        .last()
                        .map(|b| b.conditions.clone())
                        .unwrap_or_default();
                    for pattern in &args {
                        self.include(pattern, file, line_no, &current, depth);
                    }
                    // Continue the enclosing block after the included text.
                    self.open_block(current);
                }
                _ => {
                    if let Some(block) = self.blocks.last_mut() {
                        block.directives.push(Directive {
                            keyword: lower,
                            args,
                        });
                    }
                }
            }
        }
    }

    fn parse_match(
        &mut self,
        args: &[String],
        file: Option<&Path>,
        line: usize,
    ) -> Option<Vec<Criterion>> {
        let mut criteria = Vec::new();
        let mut usable = true;
        let mut iter = args.iter();
        while let Some(raw) = iter.next() {
            let (negated, name) = match raw.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, raw.as_str()),
            };
            let lname = name.to_ascii_lowercase();
            let kind = match lname.as_str() {
                "all" => Some(CriterionKind::All),
                "host" | "originalhost" | "user" | "localuser" => {
                    let Some(value) = iter.next() else {
                        self.warn(
                            file,
                            Some(line),
                            format!("Match `{name}` is missing its argument; block never matches"),
                        );
                        return None;
                    };
                    let value = value.clone();
                    Some(match lname.as_str() {
                        "host" => CriterionKind::Host(value),
                        "originalhost" => CriterionKind::OriginalHost(value),
                        "user" => CriterionKind::User(value),
                        _ => CriterionKind::LocalUser(value),
                    })
                }
                "canonical" | "final" => None,
                "exec" | "localnetwork" | "tagged" | "version" | "sessiontype" | "command" => {
                    iter.next();
                    None
                }
                _ => {
                    self.warn(
                        file,
                        Some(line),
                        format!("unknown Match criterion `{name}`; block never matches"),
                    );
                    return None;
                }
            };
            match kind {
                Some(kind) => criteria.push(Criterion { negated, kind }),
                None => {
                    usable = false;
                    self.warn(
                        file,
                        Some(line),
                        format!(
                            "Match criterion `{name}` is not supported (never executed); \
                             block treated as not matching"
                        ),
                    );
                }
            }
        }
        if !usable {
            return None;
        }
        let has_all = criteria
            .iter()
            .any(|c| matches!(c.kind, CriterionKind::All));
        if has_all && criteria.len() > 1 {
            self.warn(
                file,
                Some(line),
                "Match `all` must appear alone; block never matches",
            );
            return None;
        }
        Some(criteria)
    }

    fn include(
        &mut self,
        pattern: &str,
        file: Option<&Path>,
        line: usize,
        conditions: &[usize],
        depth: usize,
    ) {
        let Some(glob_pattern) = self.include_pattern(pattern) else {
            self.warn(
                file,
                Some(line),
                format!("Include `{pattern}`: path is not valid UTF-8; skipped"),
            );
            return;
        };
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        let paths = match glob::glob_with(&glob_pattern, opts) {
            Ok(paths) => paths,
            Err(e) => {
                self.warn(file, Some(line), format!("Include `{pattern}`: {e}"));
                return;
            }
        };
        let mut found = false;
        for entry in paths {
            let path = match entry {
                Ok(p) => p,
                Err(e) => {
                    self.warn(file, Some(line), format!("Include `{pattern}`: {e}"));
                    continue;
                }
            };
            found = true;
            if depth + 1 > MAX_INCLUDE_DEPTH {
                self.warn(
                    file,
                    Some(line),
                    format!(
                        "Include `{}` exceeds the maximum depth of {MAX_INCLUDE_DEPTH}; skipped",
                        path.display()
                    ),
                );
                continue;
            }
            let canonical = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if self.stack.contains(&canonical) {
                self.warn(
                    file,
                    Some(line),
                    format!("Include cycle through `{}`; skipped", path.display()),
                );
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    self.warn(
                        file,
                        Some(line),
                        format!("cannot read included file `{}`: {e}", path.display()),
                    );
                    continue;
                }
            };
            self.stack.push(canonical);
            self.parse_text(&text, Some(&path), conditions, depth + 1);
            self.stack.pop();
        }
        if !found {
            self.warn(
                file,
                Some(line),
                format!("Include `{pattern}` matched no files"),
            );
        }
    }

    /// Absolute glob pattern for an `Include` argument, with the base
    /// directory escaped so its characters are matched literally.
    fn include_pattern(&self, pattern: &str) -> Option<String> {
        let escaped = |p: &Path| p.to_str().map(glob::Pattern::escape);
        if pattern == "~" {
            return escaped(&self.options.home_dir);
        }
        if let Some(rest) = pattern.strip_prefix("~/") {
            return Some(format!("{}/{rest}", escaped(&self.options.home_dir)?));
        }
        if Path::new(pattern).is_absolute() {
            return Some(pattern.to_owned());
        }
        Some(format!("{}/{pattern}", escaped(&self.options.ssh_dir)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> ParseOptions {
        ParseOptions::new("/nonexistent-home", "alice")
    }

    #[test]
    fn unknown_keywords_and_bad_lines_warn() {
        let cfg = SshConfig::parse_str(
            "Bogus yes\nHost a\n  User\n  User \"open\n  Port 22\n",
            &opts(),
        );
        let msgs: Vec<_> = cfg
            .warnings()
            .iter()
            .map(|w| (w.line, w.message.clone()))
            .collect();
        assert_eq!(msgs.len(), 3, "{msgs:?}");
        assert_eq!(msgs[0].0, Some(1));
        assert!(msgs[0].1.contains("Bogus"));
        assert_eq!(msgs[1].0, Some(3));
        assert_eq!(msgs[2].0, Some(4));
        assert_eq!(cfg.resolve("a").get("port"), Some("22"));
    }

    #[test]
    fn match_criteria_validation() {
        let cfg = SshConfig::parse_str(
            "Match all host x\n  User a\nMatch host\n  User b\nMatch canonical host x\n  User c\nMatch frobnicate\n  User d\n",
            &opts(),
        );
        assert_eq!(cfg.warnings().len(), 4, "{:?}", cfg.warnings());
        assert_eq!(cfg.resolve("x").get("user"), Some("alice"));
    }

    #[test]
    fn missing_include_warns() {
        let cfg = SshConfig::parse_str("Include does-not-exist-*.conf\n", &opts());
        assert_eq!(cfg.warnings().len(), 1);
        assert!(cfg.warnings()[0].message.contains("matched no files"));
    }
}
