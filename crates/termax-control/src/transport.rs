//! Byte streams between a client and the endpoint: a Unix domain socket or
//! a Windows named pipe, both reachable only by the current OS user.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::ControlError;
use crate::paths::{Endpoint, InstanceLock};

/// A bidirectional stream.
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}

/// A connected stream.
pub type Stream = Box<dyn Io>;

/// What the OS says about the other end of an accepted connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Peer {
    /// Process id, when the OS reports it.
    pub pid: Option<u32>,
    /// User id (Unix).
    pub uid: Option<u32>,
}

#[cfg(unix)]
mod imp {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use tokio::net::{UnixListener, UnixStream};

    use super::*;
    use crate::paths::{MAX_SOCKET_PATH, check_private_socket, euid};

    /// A listening endpoint. Removes its socket file on drop.
    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        /// Bind the endpoint. The caller holds the instance lock, so a socket
        /// file left by a dead runtime is stale and is replaced.
        pub fn bind(endpoint: &Endpoint, _lock: &InstanceLock) -> Result<Self, ControlError> {
            let path = endpoint.socket_path();
            let len = path.as_os_str().len();
            if len > MAX_SOCKET_PATH {
                return Err(ControlError::PathTooLong {
                    path: path.display().to_string(),
                    len,
                    limit: MAX_SOCKET_PATH,
                });
            }
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "removed a stale control socket"),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            // The directory is 0700, so nobody else can reach the socket in
            // the moment before its own mode is set.
            let inner = UnixListener::bind(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { inner, path })
        }

        /// Wait for the next connection from this user. Connections from
        /// other users (possible only if the file modes were loosened) are
        /// closed and logged.
        pub async fn accept(&self) -> io::Result<(Stream, Peer)> {
            let me = euid();
            loop {
                let (stream, _) = self.inner.accept().await?;
                let cred = match stream.peer_cred() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!(error = %e, "control endpoint: peer credentials unavailable; connection refused");
                        continue;
                    }
                };
                if cred.uid() != me {
                    tracing::warn!(
                        peer_uid = cred.uid(),
                        peer_pid = ?cred.pid(),
                        "control endpoint refused a connection from another user"
                    );
                    continue;
                }
                let peer = Peer {
                    pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
                    uid: Some(cred.uid()),
                };
                return Ok((Box::new(stream), peer));
            }
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            if let Err(e) = std::fs::remove_file(&self.path)
                && e.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, "control socket not removed");
            }
        }
    }

    /// Connect to the endpoint after checking that the socket and its
    /// directory belong to this user.
    pub async fn connect(endpoint: &Endpoint) -> Result<Stream, ControlError> {
        let path = endpoint.socket_path();
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ControlError::NotRunning(endpoint.display()));
            }
            Err(e) => return Err(e.into()),
            Ok(_) => check_private_socket(&path)?,
        }
        match UnixStream::connect(&path).await {
            Ok(s) => Ok(Box::new(s)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                ) =>
            {
                Err(ControlError::NotRunning(endpoint.display()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(windows)]
mod imp {
    use interprocess::os::windows::named_pipe::PipeListenerOptions;
    use interprocess::os::windows::named_pipe::pipe_mode::Bytes;
    use interprocess::os::windows::named_pipe::tokio::{DuplexPipeStream, PipeListener};
    use interprocess::os::windows::security_descriptor::SecurityDescriptor;
    use widestring::U16CString;

    use super::*;

    /// Protected DACL with a single entry granting full access to the
    /// pipe's owner (the user that created it, through the OWNER RIGHTS
    /// SID); everyone else, including other users, is denied.
    const OWNER_ONLY_SDDL: &str = "D:P(A;;GA;;;OW)";

    /// A listening named pipe.
    pub struct Listener {
        inner: PipeListener<Bytes, Bytes>,
    }

    impl Listener {
        /// Create the pipe. Creating it fails if another process already
        /// holds the name (first-instance flag), so a squatter cannot
        /// receive our clients.
        pub fn bind(endpoint: &Endpoint, _lock: &InstanceLock) -> Result<Self, ControlError> {
            let sddl = U16CString::from_str(OWNER_ONLY_SDDL)
                .map_err(|e| ControlError::Protocol(e.to_string()))?;
            let sd = SecurityDescriptor::deserialize(&sddl)?;
            let inner = PipeListenerOptions::new()
                .path(endpoint.pipe_name())
                .accept_remote(false)
                .security_descriptor(Some(sd))
                .create_tokio_duplex::<Bytes>()?;
            Ok(Self { inner })
        }

        /// Wait for the next connection. The DACL already limits clients to
        /// this user; the client process id is logged.
        pub async fn accept(&self) -> io::Result<(Stream, Peer)> {
            let stream = self.inner.accept().await?;
            let pid = stream.client_process_id().ok();
            tracing::debug!(client_pid = ?pid, "control endpoint accepted a pipe client");
            Ok((Box::new(stream), Peer { pid, uid: None }))
        }
    }

    /// Connect to the pipe.
    pub async fn connect(endpoint: &Endpoint) -> Result<Stream, ControlError> {
        match DuplexPipeStream::<Bytes>::connect_by_path(endpoint.pipe_name()).await {
            Ok(s) => Ok(Box::new(s)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Err(ControlError::NotRunning(endpoint.display()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

pub use imp::{Listener, connect};
