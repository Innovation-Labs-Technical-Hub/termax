use std::fmt;
use std::path::{Path, PathBuf};

/// A non-fatal problem found while parsing, resolving or importing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Source file, or `None` for text parsed from a string.
    pub file: Option<PathBuf>,
    /// 1-based line number, when the problem is tied to a line.
    pub line: Option<usize>,
    pub message: String,
}

impl Warning {
    pub(crate) fn new(
        file: Option<&Path>,
        line: Option<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            file: file.map(Path::to_path_buf),
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.file {
            Some(p) => write!(f, "{}", p.display())?,
            None => f.write_str("<input>")?,
        }
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        write!(f, ": {}", self.message)
    }
}
