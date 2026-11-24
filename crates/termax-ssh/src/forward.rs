//! Local port forwarding (`ssh -L`).

use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, info, warn};

use crate::error::{Result, bind_error};
use crate::session::Session;

/// A running local forward. Dropping it (or calling [`ForwardHandle::close`])
/// stops the listener and tears down every tunnelled connection.
#[derive(Debug)]
pub struct ForwardHandle {
    local_addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl ForwardHandle {
    /// Address actually bound (useful when binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stop the forward and wait until the listener and all tunnels are gone.
    pub async fn close(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take()
            && let Err(e) = task.await
            && !e.is_cancelled()
        {
            warn!(error = %e, "forward task failed");
        }
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            // Aborting drops the JoinSet, which aborts every tunnel.
            task.abort();
        }
    }
}

pub(crate) async fn start(
    session: Session,
    bind: SocketAddr,
    remote_host: &str,
    remote_port: u16,
) -> Result<ForwardHandle> {
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| bind_error(bind, &e))?;
    let local_addr = listener.local_addr()?;
    let (stop_tx, stop_rx) = oneshot::channel();
    let remote_host = remote_host.to_owned();
    info!(%local_addr, remote = %format!("{remote_host}:{remote_port}"), "local forward started");
    let task = tokio::spawn(accept_loop(
        session,
        listener,
        remote_host,
        remote_port,
        stop_rx,
    ));
    Ok(ForwardHandle {
        local_addr,
        stop: Some(stop_tx),
        task: Some(task),
    })
}

async fn accept_loop(
    session: Session,
    listener: TcpListener,
    remote_host: String,
    remote_port: u16,
    mut stop: oneshot::Receiver<()>,
) {
    let mut tunnels = JoinSet::new();
    let mut closed = session.subscribe();
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = async {
                closed
                    .wait_for(|s| matches!(s, crate::ConnectionState::Closed(_)))
                    .await
                    .is_ok()
            } => {
                debug!("session closed; stopping local forward");
                break;
            }
            accepted = listener.accept() => match accepted {
                Ok((sock, peer)) => {
                    tunnels.spawn(tunnel(session.clone(), sock, peer, remote_host.clone(), remote_port));
                }
                Err(e) => {
                    // Transient (EMFILE, ECONNABORTED): log and keep serving.
                    warn!(error = %e, "local forward accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            },
            Some(_) = tunnels.join_next(), if !tunnels.is_empty() => {}
        }
    }
    tunnels.shutdown().await;
}

async fn tunnel(session: Session, mut sock: TcpStream, peer: SocketAddr, host: String, port: u16) {
    let ch = match session.open_direct_tcpip(&host, port, Some(peer)).await {
        Ok(c) => c,
        Err(e) => {
            warn!(%peer, error = %e, "direct-tcpip open failed");
            return;
        }
    };
    let mut stream = ch.into_stream();
    match tokio::io::copy_bidirectional(&mut sock, &mut stream).await {
        Ok((up, down)) => debug!(%peer, up, down, "tunnel closed"),
        Err(e) => debug!(%peer, error = %e, "tunnel ended with error"),
    }
}
