//! Headless mode: the session runtime and the control endpoint without a
//! UI. One runtime per user (the endpoint lock); it exits after a period
//! with no live sessions and no connected clients, removing its socket.
//! Sessions are opened from the shared vault ([`VaultOpener`]).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use termax_core::{Env, Host, HostId, Protocol, VaultId};
use termax_session::{ClientRef, OpenRequest, SessionInfo, SessionService, StateKind, Target};
use termax_ssh::{
    AuthMethod, ConnectContext, ConnectOptions, FixedTrust, NoPrompter, PtyRequest, TrustChoice,
    VaultVerifier,
};
use termax_vault::{HostFilter, VaultStore};
use tokio::sync::broadcast::error::RecvError;

use crate::error::ControlError;
use crate::paths::{Endpoint, InstanceLock};
use crate::protocol::OpenParams;
use crate::server::{ControlHandle, ServerConfig, SessionOpener};
use crate::transport::Listener;

/// Default idle time before a headless runtime exits.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// A running headless endpoint: holds the per-user lock and serves the
/// socket until [`Headless::shutdown`].
pub struct Headless {
    control: ControlHandle,
    _lock: InstanceLock,
    service: SessionService,
}

impl Headless {
    /// Take the per-user lock, bind the endpoint and start serving it on
    /// the runtime behind `service`. Fails with
    /// [`ControlError::AlreadyRunning`] when another runtime holds it.
    pub fn start(
        endpoint: &Endpoint,
        service: SessionService,
        config: ServerConfig,
    ) -> Result<Self, ControlError> {
        let lock = InstanceLock::acquire(endpoint)?;
        let listener = {
            let _rt = service.handle().enter();
            Listener::bind(endpoint, &lock)?
        };
        let control = crate::server::start(service.clone(), listener, config);
        Ok(Self {
            control,
            _lock: lock,
            service,
        })
    }

    /// The endpoint (approvals hook, clients, audit).
    pub fn control(&self) -> &ControlHandle {
        &self.control
    }

    /// Return once there have been no live sessions and no connected
    /// clients for `idle` (on the runtime clock).
    pub async fn wait_until_idle(&self, idle: Duration) {
        wait_until_idle(&self.service, &self.control, idle).await;
    }

    /// Stop serving and remove the socket. Sessions are left to the
    /// caller (`SessionRuntime::quit`).
    pub async fn shutdown(self) {
        let Self { control, _lock, .. } = self;
        control.shutdown().await;
    }
}

fn busy(service: &SessionService, clients: usize) -> bool {
    clients > 0
        || service
            .list()
            .iter()
            .any(|s| s.state.kind() != StateKind::Closed)
}

/// See [`Headless::wait_until_idle`].
pub async fn wait_until_idle(service: &SessionService, control: &ControlHandle, idle: Duration) {
    let clock = service.clock();
    let mut events = service.subscribe();
    let mut connected = control.watch_connected();
    loop {
        let clients = *connected.borrow_and_update();
        if busy(service, clients) {
            tokio::select! {
                r = events.recv() => if matches!(r, Err(RecvError::Closed)) { return; },
                r = connected.changed() => if r.is_err() { return; },
            }
            continue;
        }
        tokio::select! {
            () = clock.sleep(idle) => return,
            r = events.recv() => if matches!(r, Err(RecvError::Closed)) { return; },
            r = connected.changed() => if r.is_err() { return; },
        }
    }
}

/// Opens sessions for saved hosts of one vault: address, port, `user` and
/// `identityfile` settings from the host, the ssh-agent, and the vault's
/// known hosts. Unknown host keys are rejected (no one can be asked).
pub struct VaultOpener {
    store: Arc<Mutex<VaultStore>>,
    vault_id: VaultId,
    local_user: String,
    home: Option<PathBuf>,
}

impl VaultOpener {
    /// An opener for `vault_id` in `store`; `local_user` is the default
    /// remote user name.
    pub fn new(store: Arc<Mutex<VaultStore>>, vault_id: VaultId, local_user: String) -> Self {
        Self {
            store,
            vault_id,
            local_user,
            home: directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()),
        }
    }

    fn hosts(&self) -> Result<Vec<Host>, String> {
        let store = self
            .store
            .lock()
            .map_err(|_| "vault unavailable".to_owned())?;
        store
            .list_hosts(self.vault_id, &HostFilter::default())
            .map_err(|e| format!("vault: {e}"))
    }

    fn host_by_id(&self, id: HostId) -> Option<Host> {
        self.hosts().ok()?.into_iter().find(|h| h.id == id)
    }

    fn expand(&self, path: &str) -> PathBuf {
        match (path.strip_prefix("~/"), &self.home) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => PathBuf::from(path),
        }
    }
}

impl SessionOpener for VaultOpener {
    fn open_request(&self, params: &OpenParams, owner: ClientRef) -> Result<OpenRequest, String> {
        let host = self
            .hosts()?
            .into_iter()
            .find(|h| h.label.eq_ignore_ascii_case(params.host.trim()))
            .ok_or_else(|| format!("no saved host named {:?}", params.host))?;
        if host.protocol != Protocol::Ssh {
            return Err(format!("{} is not an SSH host", host.label));
        }
        let user = host
            .settings
            .get("user")
            .map_or_else(|| self.local_user.clone(), str::to_owned);
        let mut opts = ConnectOptions::new(host.address.clone(), user);
        opts.port = host.effective_port().unwrap_or(22);
        let mut auth = Vec::new();
        if let Some(v) = host.settings.get("identityfile") {
            let files = termax_interop::decode_setting("identityfile", v)
                .map_err(|e| format!("identityfile of {}: {e}", host.label))?;
            for args in files {
                if let Some(path) = args.first() {
                    auth.push(AuthMethod::PublicKeyFile {
                        path: self.expand(path),
                        passphrase: None,
                    });
                }
            }
        }
        auth.push(AuthMethod::Agent { socket: None });
        opts.auth = auth;
        let context = ConnectContext {
            verifier: Arc::new(VaultVerifier::new(Arc::clone(&self.store))),
            trust: Arc::new(FixedTrust(TrustChoice::Reject)),
            prompter: Arc::new(NoPrompter),
        };
        let mut req = OpenRequest::new(opts, context, owner);
        req.target = Target::Host(host.id);
        req.pty = PtyRequest {
            term: "xterm-256color".into(),
            cols: u32::from(params.cols.unwrap_or(80).max(1)),
            rows: u32::from(params.rows.unwrap_or(24).max(1)),
        };
        Ok(req)
    }

    fn host_context(&self, info: &SessionInfo) -> (Env, Vec<String>) {
        match info.target {
            Target::Host(id) => self
                .host_by_id(id)
                .map_or((Env::None, Vec::new()), |h| (h.env, h.tags)),
            Target::AdHoc => (Env::None, Vec::new()),
        }
    }
}
