//! Interactive shell channels.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use russh::client::Msg;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Sig};
use tracing::debug;

use crate::error::{Error, Result};

/// How a remote command or shell ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitStatus {
    /// Exited with this code.
    Code(u32),
    /// Killed by a signal (name without `SIG`, e.g. `TERM`).
    Signal {
        /// Signal name without `SIG`.
        name: String,
        /// Whether a core dump was written.
        core_dumped: bool,
        /// Server-provided error message (may be empty).
        message: String,
    },
}

impl ExitStatus {
    pub(crate) fn from_signal(sig: &Sig, core_dumped: bool, message: String) -> Self {
        ExitStatus::Signal {
            name: sig_name(sig),
            core_dumped,
            message,
        }
    }
}

/// A signal to deliver to the remote process (RFC 4254 §6.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// `SIGABRT`.
    Abrt,
    /// `SIGALRM`.
    Alrm,
    /// `SIGFPE`.
    Fpe,
    /// `SIGHUP`.
    Hup,
    /// `SIGILL`.
    Ill,
    /// `SIGINT` (Ctrl-C).
    Int,
    /// `SIGKILL`.
    Kill,
    /// `SIGPIPE`.
    Pipe,
    /// `SIGQUIT`.
    Quit,
    /// `SIGSEGV`.
    Segv,
    /// `SIGTERM`.
    Term,
    /// `SIGUSR1`.
    Usr1,
    /// Any other signal name, without the `SIG` prefix.
    Other(String),
}

impl Signal {
    fn to_sig(&self) -> Sig {
        match self {
            Signal::Abrt => Sig::ABRT,
            Signal::Alrm => Sig::ALRM,
            Signal::Fpe => Sig::FPE,
            Signal::Hup => Sig::HUP,
            Signal::Ill => Sig::ILL,
            Signal::Int => Sig::INT,
            Signal::Kill => Sig::KILL,
            Signal::Pipe => Sig::PIPE,
            Signal::Quit => Sig::QUIT,
            Signal::Segv => Sig::SEGV,
            Signal::Term => Sig::TERM,
            Signal::Usr1 => Sig::USR1,
            Signal::Other(s) => Sig::Custom(s.clone()),
        }
    }
}

fn sig_name(sig: &Sig) -> String {
    match sig {
        Sig::ABRT => "ABRT",
        Sig::ALRM => "ALRM",
        Sig::FPE => "FPE",
        Sig::HUP => "HUP",
        Sig::ILL => "ILL",
        Sig::INT => "INT",
        Sig::KILL => "KILL",
        Sig::PIPE => "PIPE",
        Sig::QUIT => "QUIT",
        Sig::SEGV => "SEGV",
        Sig::TERM => "TERM",
        Sig::USR1 => "USR1",
        Sig::Custom(s) => return s.clone(),
    }
    .to_owned()
}

/// Write side shared by both halves; closes the channel when the last half
/// is dropped so an abandoned tab does not leave a remote shell running.
struct WriteInner {
    half: Option<ChannelWriteHalf<Msg>>,
}

impl WriteInner {
    fn get(&self) -> &ChannelWriteHalf<Msg> {
        // Only `Drop` takes the half.
        self.half
            .as_ref()
            .unwrap_or_else(|| unreachable!("write half taken outside drop"))
    }
}

impl Drop for WriteInner {
    fn drop(&mut self) {
        if let (Some(half), Ok(rt)) = (self.half.take(), tokio::runtime::Handle::try_current()) {
            rt.spawn(async move {
                if let Err(e) = half.close().await {
                    debug!(error = %e, "channel close on drop failed");
                }
            });
        }
    }
}

type ExitSlot = Arc<Mutex<Option<ExitStatus>>>;

fn exit_of(slot: &ExitSlot) -> Option<ExitStatus> {
    slot.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

/// A shell on a pseudo-terminal. Use it directly, or [`ShellChannel::split`]
/// it to read and write from different tasks.
pub struct ShellChannel {
    reader: ShellReader,
    writer: ShellWriter,
}

/// Read side of a [`ShellChannel`].
pub struct ShellReader {
    read: ChannelReadHalf,
    pending: VecDeque<Vec<u8>>,
    exit: ExitSlot,
    done: bool,
    _write: Arc<WriteInner>,
}

/// Write side of a [`ShellChannel`]; cloneable.
#[derive(Clone)]
pub struct ShellWriter {
    write: Arc<WriteInner>,
    exit: ExitSlot,
}

impl ShellChannel {
    /// Wait for the pty and shell request replies, then wrap the channel.
    pub(crate) async fn start(ch: Channel<Msg>) -> Result<Self> {
        let (mut read, write) = ch.split();
        let exit: ExitSlot = Arc::default();
        let mut pending = VecDeque::new();
        let mut replies = 0;
        while replies < 2 {
            match read.wait().await {
                Some(ChannelMsg::Success) => replies += 1,
                Some(ChannelMsg::Failure) => {
                    let what = if replies == 0 { "pty" } else { "shell" };
                    // Best effort: the channel is useless now.
                    let _ = write.close().await;
                    return Err(Error::ChannelOpen(format!("server refused {what} request")));
                }
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    pending.push_back(data.to_vec());
                }
                None | Some(ChannelMsg::Close) => {
                    return Err(Error::ChannelOpen(
                        "channel closed during shell setup".into(),
                    ));
                }
                Some(_) => {}
            }
        }
        let write = Arc::new(WriteInner { half: Some(write) });
        Ok(Self {
            reader: ShellReader {
                read,
                pending,
                exit: Arc::clone(&exit),
                done: false,
                _write: Arc::clone(&write),
            },
            writer: ShellWriter { write, exit },
        })
    }

    /// Next chunk of terminal output (stdout and stderr merged, as on a
    /// terminal). `None` once the channel is closed.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        self.reader.read().await
    }

    /// Send input bytes.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.writer.write(data).await
    }

    /// Tell the server the terminal size changed.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        self.writer.resize(cols, rows).await
    }

    /// Deliver a signal to the remote process.
    pub async fn signal(&self, sig: Signal) -> Result<()> {
        self.writer.signal(sig).await
    }

    /// Send a BREAK (RFC 4335). See [`ShellWriter::send_break`].
    pub async fn send_break(&self, millis: u32) -> Result<()> {
        self.writer.send_break(millis).await
    }

    /// Signal end of input.
    pub async fn eof(&self) -> Result<()> {
        self.writer.eof().await
    }

    /// Close the channel.
    pub async fn close(&self) -> Result<()> {
        self.writer.close().await
    }

    /// Exit status, once the server has sent it.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        exit_of(&self.writer.exit)
    }

    /// Split into independently owned read and write halves.
    pub fn split(self) -> (ShellReader, ShellWriter) {
        (self.reader, self.writer)
    }
}

impl ShellReader {
    /// Next chunk of terminal output; `None` once the channel is closed.
    pub async fn read(&mut self) -> Option<Vec<u8>> {
        if let Some(b) = self.pending.pop_front() {
            return Some(b);
        }
        if self.done {
            return None;
        }
        loop {
            match self.read.wait().await {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    return Some(data.to_vec());
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    *self.exit.lock().unwrap_or_else(|p| p.into_inner()) =
                        Some(ExitStatus::Code(exit_status));
                }
                Some(ChannelMsg::ExitSignal {
                    signal_name,
                    core_dumped,
                    error_message,
                    ..
                }) => {
                    *self.exit.lock().unwrap_or_else(|p| p.into_inner()) = Some(
                        ExitStatus::from_signal(&signal_name, core_dumped, error_message),
                    );
                }
                None | Some(ChannelMsg::Close) => {
                    self.done = true;
                    return None;
                }
                Some(_) => {}
            }
        }
    }

    /// Exit status, once the server has sent it.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        exit_of(&self.exit)
    }
}

impl ShellWriter {
    /// Send input bytes.
    pub async fn write(&self, data: &[u8]) -> Result<()> {
        self.write
            .get()
            .data_bytes(data.to_vec())
            .await
            .map_err(Error::from)
    }

    /// Tell the server the terminal size changed.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidConfig(
                "terminal size must be non-zero".into(),
            ));
        }
        self.write
            .get()
            .window_change(cols, rows, 0, 0)
            .await
            .map_err(Error::from)
    }

    /// Deliver a signal to the remote process.
    pub async fn signal(&self, sig: Signal) -> Result<()> {
        self.write
            .get()
            .signal(sig.to_sig())
            .await
            .map_err(Error::from)
    }

    /// Send a BREAK of `millis` ms (RFC 4335, used by serial consoles).
    ///
    /// Currently always returns [`Error::Unsupported`]: the russh backend
    /// has no way to send the `break` channel request. The method exists so
    /// callers are written against the final API.
    pub async fn send_break(&self, millis: u32) -> Result<()> {
        let _ = millis;
        Err(Error::Unsupported(
            "break request (RFC 4335) is not exposed by the SSH backend",
        ))
    }

    /// Signal end of input.
    pub async fn eof(&self) -> Result<()> {
        self.write.get().eof().await.map_err(Error::from)
    }

    /// Close the channel.
    pub async fn close(&self) -> Result<()> {
        self.write.get().close().await.map_err(Error::from)
    }

    /// Exit status, once the reader has seen it.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        exit_of(&self.exit)
    }
}
