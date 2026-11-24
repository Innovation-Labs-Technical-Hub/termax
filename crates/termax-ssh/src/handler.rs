//! The russh client handler: host-key trust, negotiated-algorithm capture and
//! connection-state publication.

use std::sync::{Arc, Mutex};

use russh::client::{DisconnectReason, Session as RusshSession};
use russh::keys::PublicKeyOrCertificate;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::deadline::PromptClock;
use crate::error::Error;
use crate::session::{CloseReason, ConnectionState, Negotiated};
use crate::trust::{HostKeyInfo, HostKeyStatus, HostKeyVerifier, TrustChoice, TrustDecision};

/// State shared between the handler (inside the russh task) and the session.
pub(crate) struct Shared {
    pub negotiated: Mutex<Option<Negotiated>>,
    pub state: watch::Sender<ConnectionState>,
}

impl Shared {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            negotiated: Mutex::new(None),
            state: watch::Sender::new(ConnectionState::Connected),
        })
    }

    pub(crate) fn close(&self, reason: CloseReason) {
        self.state.send_if_modified(|s| {
            if matches!(s, ConnectionState::Connected) {
                *s = ConnectionState::Closed(reason);
                true
            } else {
                false
            }
        });
    }

    pub(crate) fn negotiated(&self) -> Option<Negotiated> {
        self.negotiated
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

pub(crate) struct ClientHandler {
    pub host: String,
    pub port: u16,
    pub verifier: Arc<dyn HostKeyVerifier>,
    pub trust: Arc<dyn TrustDecision>,
    pub clock: Arc<PromptClock>,
    pub shared: Arc<Shared>,
}

impl ClientHandler {
    async fn check(&self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            // We never advertise certificate host-key algorithms.
            return Err(Error::Protocol(
                "server presented a host certificate that was not requested".into(),
            ));
        };
        let info = HostKeyInfo::new(&self.host, self.port, key);
        match self.verifier.verify(&info).await? {
            HostKeyStatus::Trusted => Ok(true),
            HostKeyStatus::Revoked => {
                warn!(host = %info.host_port, fp = %info.fingerprint, "revoked host key");
                Err(Error::HostKeyRevoked {
                    host_port: info.host_port,
                    fingerprint: info.fingerprint,
                })
            }
            HostKeyStatus::Changed {
                old_key_type,
                old,
                new,
            } => {
                warn!(host = %info.host_port, %old, %new, "host key changed");
                Err(Error::HostKeyChanged {
                    host_port: info.host_port,
                    old_key_type,
                    old_fingerprint: old,
                    new_key_type: info.key_type,
                    new_fingerprint: new,
                })
            }
            HostKeyStatus::Unknown { .. } => {
                match self.clock.pause(self.trust.unknown_host(&info)).await {
                    TrustChoice::AcceptAndSave => {
                        self.verifier.save(&info).await?;
                        info!(host = %info.host_port, fp = %info.fingerprint, "host key saved");
                        Ok(true)
                    }
                    TrustChoice::AcceptOnce => Ok(true),
                    TrustChoice::Reject => Err(Error::HostKeyRejected {
                        host_port: info.host_port,
                        fingerprint: info.fingerprint,
                    }),
                }
            }
        }
    }
}

impl russh::client::Handler for ClientHandler {
    type Error = Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Error> {
        self.check(key).await
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        session: &mut RusshSession,
    ) -> Result<(), Error> {
        let n = Negotiated::from_names(
            names,
            String::from_utf8_lossy(session.remote_sshid()).into_owned(),
        );
        *self
            .shared
            .negotiated
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(n);
        Ok(())
    }

    async fn disconnected(&mut self, reason: DisconnectReason<Error>) -> Result<(), Error> {
        match reason {
            DisconnectReason::ReceivedDisconnect(info) => {
                self.shared.close(CloseReason::ServerDisconnect {
                    message: info.message,
                });
                Ok(())
            }
            DisconnectReason::Error(Error::KeepaliveTimeout) => {
                warn!(host = %self.host, "keepalive timeout; connection declared dead");
                self.shared.close(CloseReason::KeepaliveTimeout);
                Err(Error::KeepaliveTimeout)
            }
            DisconnectReason::Error(e) => {
                self.shared.close(CloseReason::Error(e.to_string()));
                Err(e)
            }
        }
    }
}

impl Drop for ClientHandler {
    fn drop(&mut self) {
        // The russh task has ended; nothing else will update the state.
        self.shared.close(CloseReason::Local);
    }
}
