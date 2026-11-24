use std::io;
use std::path::PathBuf;

/// Errors that prevent a whole file from being read.
///
/// Problems inside a file (bad lines, unsupported keywords, broken includes)
/// are reported as warnings instead, so one bad line never blocks an import.
#[derive(Debug, thiserror::Error)]
pub enum InteropError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}
