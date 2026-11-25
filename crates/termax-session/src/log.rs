//! The connection log: every lifecycle transition, with codes and detail.
//! Entries never contain secrets or terminal content.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::model::SessionId;

/// One log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Wall-clock time.
    pub at: SystemTime,
    /// Session.
    pub session: SessionId,
    /// `user@host:port`.
    pub address: String,
    /// What happened, e.g. `Connecting -> Disconnected(TMX-NET-002): …`.
    pub message: String,
}

/// Where connection log entries go.
pub trait ConnectionLog: Send + Sync + 'static {
    /// Record an entry. Must be cheap: it runs on I/O workers.
    fn record(&self, entry: &LogEntry);

    /// Write buffered entries out. Runs on the blocking pool.
    fn flush(&self) -> io::Result<()>;
}

/// Discards entries (tracing still sees them).
#[derive(Debug, Default)]
pub struct NullLog;

impl ConnectionLog for NullLog {
    fn record(&self, _: &LogEntry) {}
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

/// Keeps entries in memory (tests, diagnostics view).
#[derive(Debug, Default)]
pub struct MemoryLog {
    entries: Mutex<Vec<LogEntry>>,
}

impl MemoryLog {
    /// Entries so far.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

impl ConnectionLog for MemoryLog {
    fn record(&self, entry: &LogEntry) {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(entry.clone());
    }
    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

/// Appends one line per entry to a file, buffered; [`ConnectionLog::flush`]
/// writes the buffer out and syncs.
#[derive(Debug)]
pub struct FileLog {
    out: Mutex<BufWriter<File>>,
}

impl FileLog {
    /// Open (or create) `path` for appending.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            out: Mutex::new(BufWriter::with_capacity(64 * 1024, file)),
        })
    }
}

impl ConnectionLog for FileLog {
    fn record(&self, e: &LogEntry) {
        let secs =
            e.at.duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0.0, |d| d.as_secs_f64());
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(err) = writeln!(out, "{secs:.3} {} {} {}", e.session, e.address, e.message) {
            tracing::warn!(error = %err, "connection log write failed");
        }
    }

    fn flush(&self) -> io::Result<()> {
        let mut out = self.out.lock().unwrap_or_else(|p| p.into_inner());
        out.flush()?;
        out.get_ref().sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_log_appends_lines_and_flushes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections.log");
        let log = FileLog::open(&path).unwrap();
        let entry = LogEntry {
            at: SystemTime::now(),
            session: SessionId::new(),
            address: "me@example:22".into(),
            message: "Idle -> Resolving".into(),
        };
        log.record(&entry);
        log.record(&entry);
        log.flush().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains("me@example:22 Idle -> Resolving"));
    }
}
