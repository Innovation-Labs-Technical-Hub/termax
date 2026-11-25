//! Connection setup (direct or through jump hosts) and the session handle.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use russh::client::{Handle, Msg};
use russh::{Channel, ChannelMsg};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tracing::{debug, info};

use crate::auth::{AuthRequest, Prompter, authenticate};
use crate::channel::{ExitStatus, ShellChannel};
use crate::deadline::PromptClock;
use crate::error::{Error, Result};
use crate::forward::{self, ForwardHandle};
use crate::handler::{ClientHandler, Shared};
use crate::options::{AuthMethod, ConnectOptions, PtyRequest};
use crate::quirks::quirks_for;
use crate::trust::{HostKeyVerifier, TrustDecision, host_port};

/// Algorithms negotiated for a connection (for the status bar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Negotiated {
    /// Key exchange, e.g. `mlkem768x25519-sha256`.
    pub kex: String,
    /// Host key signature algorithm, e.g. `ssh-ed25519`.
    pub host_key: String,
    /// Cipher, e.g. `chacha20-poly1305@openssh.com`.
    pub cipher: String,
    /// MAC client→server (meaningless for AEAD ciphers).
    pub mac_client_to_server: String,
    /// MAC server→client (meaningless for AEAD ciphers).
    pub mac_server_to_client: String,
    /// Strict key exchange (Terrapin countermeasure) is in effect.
    pub strict_kex: bool,
    /// The key exchange is a post-quantum hybrid (ML-KEM or sntrup).
    pub post_quantum: bool,
    /// Server identification string, e.g. `SSH-2.0-OpenSSH_10.2p1`.
    pub server_id: String,
}

impl Negotiated {
    pub(crate) fn from_names(n: &russh::Names, server_id: String) -> Self {
        let kex = n.kex.as_ref().to_owned();
        Self {
            post_quantum: is_post_quantum(&kex),
            kex,
            host_key: n.key.as_str().to_owned(),
            cipher: n.cipher.as_ref().to_owned(),
            mac_client_to_server: n.client_mac.as_ref().to_owned(),
            mac_server_to_client: n.server_mac.as_ref().to_owned(),
            strict_kex: n.strict_kex(),
            server_id: server_id.trim_end().to_owned(),
        }
    }
}

/// Whether a key exchange name is a post-quantum hybrid.
pub fn is_post_quantum(kex: &str) -> bool {
    kex.starts_with("mlkem") || kex.starts_with("sntrup")
}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// The server stopped answering keepalives.
    KeepaliveTimeout,
    /// The server sent a disconnect message.
    ServerDisconnect {
        /// The server's description (may be empty).
        message: String,
    },
    /// A transport or protocol error.
    Error(String),
    /// Closed locally (disconnect called or all handles dropped).
    Local,
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CloseReason::KeepaliveTimeout => f.write_str("server not responding to keepalives"),
            CloseReason::ServerDisconnect { message } if message.is_empty() => {
                f.write_str("disconnected by server")
            }
            CloseReason::ServerDisconnect { message } => {
                write!(f, "disconnected by server: {message}")
            }
            CloseReason::Error(e) => f.write_str(e),
            CloseReason::Local => f.write_str("closed locally"),
        }
    }
}

/// Connection liveness, published through [`Session::subscribe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// Authenticated and usable.
    Connected,
    /// Closed; no further channels can be opened.
    Closed(CloseReason),
}

/// Callbacks used while connecting.
#[derive(Clone)]
pub struct ConnectContext {
    /// Host-key trust store.
    pub verifier: Arc<dyn HostKeyVerifier>,
    /// Asked about unknown host keys.
    pub trust: Arc<dyn TrustDecision>,
    /// Asked for passphrases, passwords and keyboard-interactive answers.
    pub prompter: Arc<dyn Prompter>,
}

/// Output of [`Session::exec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    /// Standard output.
    pub stdout: Vec<u8>,
    /// Standard error.
    pub stderr: Vec<u8>,
    /// How the command ended; `None` if the server did not say.
    pub exit: Option<ExitStatus>,
}

impl ExecOutput {
    /// Exit code, if the command exited normally.
    pub fn exit_status(&self) -> Option<u32> {
        match self.exit {
            Some(ExitStatus::Code(c)) => Some(c),
            _ => None,
        }
    }
}

/// An authenticated SSH connection. Cheap to clone; every clone shares the
/// same transport, so new channels (duplicate tab, exec, forwards) need no
/// new TCP connection or key exchange. The connection closes when the last
/// clone (and every channel/forward derived from it) is dropped, or on
/// [`Session::disconnect`].
#[derive(Clone)]
pub struct Session {
    inner: Arc<Inner>,
}

struct Inner {
    handle: Handle<ClientHandler>,
    shared: Arc<Shared>,
    negotiated: Negotiated,
    target: String,
    /// Keeps the previous hop alive for the life of this connection.
    _via: Option<Session>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("target", &self.inner.target)
            .field("negotiated", &self.inner.negotiated)
            .finish_non_exhaustive()
    }
}

struct Hop<'a> {
    host: &'a str,
    port: u16,
    user: &'a str,
    auth: &'a [AuthMethod],
}

impl Session {
    /// Connect, verify the host key(s), and authenticate. With jump hosts,
    /// each hop is reached through a `direct-tcpip` channel of the previous
    /// one and has its host key verified independently.
    pub async fn connect(opts: &ConnectOptions, ctx: &ConnectContext) -> Result<Session> {
        Self::connect_inner(opts, ctx, None).await
    }

    /// Like [`Session::connect`], but the first hop (the first jump host, or
    /// the target when there are none) runs over `tcp`, a stream the caller
    /// already connected to it. Callers use this to do their own name
    /// resolution and address racing (Happy Eyeballs). Host keys are still
    /// verified under the `host:port` in the options, not the peer address.
    pub async fn connect_stream(
        opts: &ConnectOptions,
        ctx: &ConnectContext,
        tcp: TcpStream,
    ) -> Result<Session> {
        Self::connect_inner(opts, ctx, Some(tcp)).await
    }

    async fn connect_inner(
        opts: &ConnectOptions,
        ctx: &ConnectContext,
        mut first_stream: Option<TcpStream>,
    ) -> Result<Session> {
        opts.validate()?;
        let config = Arc::new(russh::client::Config {
            preferred: opts.crypto.to_preferred()?,
            window_size: opts.window_size,
            maximum_packet_size: opts.max_packet_size,
            keepalive_interval: opts.keepalive_interval,
            keepalive_max: opts.keepalive_max_missed,
            inactivity_timeout: None,
            nodelay: true,
            ..Default::default()
        });

        let hops = opts
            .jump_hosts
            .iter()
            .map(|h| Hop {
                host: &h.host,
                port: h.port,
                user: &h.user,
                auth: &h.auth,
            })
            .chain(std::iter::once(Hop {
                host: &opts.host,
                port: opts.port,
                user: &opts.user,
                auth: &opts.auth,
            }));

        let mut via: Option<Session> = None;
        for hop in hops {
            let clock = Arc::new(PromptClock::default());
            let target = host_port(hop.host, hop.port);
            let fut = connect_hop(
                &config,
                &hop,
                via.take(),
                first_stream.take(),
                ctx,
                Arc::clone(&clock),
                opts,
            );
            let s = clock.run("connect", opts.connect_timeout, fut).await?;
            info!(
                target = %target,
                kex = %s.inner.negotiated.kex,
                cipher = %s.inner.negotiated.cipher,
                strict_kex = s.inner.negotiated.strict_kex,
                "ssh connected"
            );
            via = Some(s);
        }
        via.ok_or_else(|| Error::InvalidConfig("no hops".into()))
    }

    /// Negotiated algorithms of this hop (the final target).
    pub fn negotiated(&self) -> &Negotiated {
        &self.inner.negotiated
    }

    /// `host:port` of the target.
    pub fn target(&self) -> &str {
        &self.inner.target
    }

    /// Current connection state.
    pub fn state(&self) -> ConnectionState {
        self.inner.shared.state.borrow().clone()
    }

    /// Watch connection state changes (e.g. keepalive death).
    pub fn subscribe(&self) -> watch::Receiver<ConnectionState> {
        self.inner.shared.state.subscribe()
    }

    /// Wait until the connection closes and return why.
    pub async fn closed(&self) -> CloseReason {
        let mut rx = self.subscribe();
        loop {
            if let ConnectionState::Closed(r) = &*rx.borrow_and_update() {
                return r.clone();
            }
            if rx.changed().await.is_err() {
                return CloseReason::Local;
            }
        }
    }

    /// Whether the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.inner.handle.is_closed() || matches!(self.state(), ConnectionState::Closed(_))
    }

    /// Send a disconnect to the server. Other clones become unusable.
    pub async fn disconnect(&self) -> Result<()> {
        self.inner.shared.close(CloseReason::Local);
        self.inner
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await
            .map_err(Error::from)
    }

    fn ensure_open(&self) -> Result<()> {
        match self.state() {
            ConnectionState::Closed(r) => Err(Error::Closed(r)),
            ConnectionState::Connected => Ok(()),
        }
    }

    async fn open_session_channel(&self) -> Result<Channel<Msg>> {
        self.ensure_open()?;
        self.inner
            .handle
            .channel_open_session()
            .await
            .map_err(|e| self.channel_error(e))
    }

    pub(crate) async fn open_direct_tcpip(
        &self,
        host: &str,
        port: u16,
        originator: Option<SocketAddr>,
    ) -> Result<Channel<Msg>> {
        self.ensure_open()?;
        let (oip, oport) = originator.map_or_else(
            || ("127.0.0.1".to_owned(), 0),
            |a| (a.ip().to_string(), u32::from(a.port())),
        );
        self.inner
            .handle
            .channel_open_direct_tcpip(host, u32::from(port), oip, oport)
            .await
            .map_err(|e| self.channel_error(e))
    }

    fn channel_error(&self, e: russh::Error) -> Error {
        match self.state() {
            ConnectionState::Closed(r) => Error::Closed(r),
            ConnectionState::Connected => Error::from(e),
        }
    }

    /// Run `command` on a new channel and collect its output. Output is
    /// buffered in memory; use [`Session::open_shell`] for streaming.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput> {
        let mut ch = self.open_session_channel().await?;
        ch.exec(true, command).await?;
        let mut out = ExecOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit: None,
        };
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => {
                    out.exit = Some(ExitStatus::Code(exit_status));
                }
                ChannelMsg::ExitSignal {
                    signal_name,
                    core_dumped,
                    error_message,
                    ..
                } => {
                    out.exit = Some(ExitStatus::from_signal(
                        &signal_name,
                        core_dumped,
                        error_message,
                    ))
                }
                ChannelMsg::Failure => {
                    return Err(Error::ChannelOpen("server refused exec request".into()));
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        debug!(command_len = command.len(), exit = ?out.exit, "exec finished");
        Ok(out)
    }

    /// Open an interactive shell with a pseudo-terminal.
    pub async fn open_shell(&self, pty: &PtyRequest) -> Result<ShellChannel> {
        if pty.term.is_empty() || pty.cols == 0 || pty.rows == 0 {
            return Err(Error::InvalidConfig(
                "pty needs a TERM and non-zero size".into(),
            ));
        }
        let ch = self.open_session_channel().await?;
        ch.request_pty(true, &pty.term, pty.cols, pty.rows, 0, 0, &[])
            .await?;
        ch.request_shell(true).await?;
        ShellChannel::start(ch).await
    }

    /// Local port forwarding (`-L`): listen on `bind`, and tunnel each
    /// accepted connection to `remote_host:remote_port` as seen from the
    /// server. Stops when the returned handle is dropped or closed.
    pub async fn forward_local(
        &self,
        bind: SocketAddr,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<ForwardHandle> {
        self.ensure_open()?;
        forward::start(self.clone(), bind, remote_host, remote_port).await
    }
}

async fn connect_hop(
    config: &Arc<russh::client::Config>,
    hop: &Hop<'_>,
    via: Option<Session>,
    stream: Option<TcpStream>,
    ctx: &ConnectContext,
    clock: Arc<PromptClock>,
    opts: &ConnectOptions,
) -> Result<Session> {
    let target = host_port(hop.host, hop.port);
    let shared = Shared::new();
    let handler = ClientHandler {
        host: hop.host.to_owned(),
        port: hop.port,
        verifier: Arc::clone(&ctx.verifier),
        trust: Arc::clone(&ctx.trust),
        clock: Arc::clone(&clock),
        shared: Arc::clone(&shared),
    };
    let connect_err = |reason: String| Error::Connect {
        target: target.clone(),
        reason,
    };

    let mut handle = match &via {
        None => {
            let tcp = match stream {
                Some(tcp) => tcp,
                None => TcpStream::connect((hop.host, hop.port))
                    .await
                    .map_err(|e| connect_err(e.to_string()))?,
            };
            if let Err(e) = tcp.set_nodelay(true) {
                debug!(error = %e, "set_nodelay failed");
            }
            russh::client::connect_stream(Arc::clone(config), tcp, handler).await?
        }
        Some(prev) => {
            let ch = prev
                .open_direct_tcpip(hop.host, hop.port, None)
                .await
                .map_err(|e| connect_err(format!("via {}: {e}", prev.target())))?;
            russh::client::connect_stream(Arc::clone(config), ch.into_stream(), handler).await?
        }
    };

    let negotiated = shared
        .negotiated()
        .ok_or_else(|| Error::Protocol("key exchange finished without algorithms".into()))?;
    if !negotiated.strict_kex {
        // We always offer strict kex; servers released before the Terrapin
        // fix do not, and the UI can show this via `Negotiated::strict_kex`.
        tracing::warn!(target = %target, "server does not support strict key exchange");
    }

    authenticate(
        &mut handle,
        AuthRequest {
            user: hop.user,
            host_port: &target,
            methods: hop.auth,
            prompter: ctx.prompter.as_ref(),
            clock: &clock,
            quirks: quirks_for(&negotiated.server_id),
            allow_trivial: opts.allow_trivial_auth,
        },
    )
    .await?;

    Ok(Session {
        inner: Arc::new(Inner {
            handle,
            shared,
            negotiated,
            target,
            _via: via,
        }),
    })
}
