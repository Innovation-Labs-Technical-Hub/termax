//! Test-only helpers on top of the shared sshd fixture.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use termax_ssh::{
    BoxFuture, HostKeyInfo, HostKeyStatus, HostKeyVerifier, KbdInteractiveRequest, MemoryVerifier,
    Prompter, TrustChoice, TrustDecision, Zeroizing,
};

pub use termax_sshd_fixture::*;

/// Upper bound for any single test body.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Trust decision that records how often it was asked.
pub struct CountingTrust {
    pub choice: TrustChoice,
    pub asked: AtomicUsize,
}

impl CountingTrust {
    pub fn new(choice: TrustChoice) -> Arc<Self> {
        Arc::new(Self {
            choice,
            asked: AtomicUsize::new(0),
        })
    }
    pub fn count(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl TrustDecision for CountingTrust {
    fn unknown_host<'a>(&'a self, _key: &'a HostKeyInfo) -> BoxFuture<'a, TrustChoice> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let c = self.choice;
        Box::pin(async move { c })
    }
}

/// Verifier wrapper that records every key it was asked about.
pub struct RecordingVerifier {
    pub inner: Arc<MemoryVerifier>,
    pub seen: std::sync::Mutex<Vec<HostKeyInfo>>,
}

impl HostKeyVerifier for RecordingVerifier {
    fn verify<'a>(
        &'a self,
        key: &'a HostKeyInfo,
    ) -> BoxFuture<'a, termax_ssh::Result<HostKeyStatus>> {
        self.seen.lock().unwrap().push(key.clone());
        self.inner.verify(key)
    }
    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, termax_ssh::Result<()>> {
        self.inner.save(key)
    }
}

/// Prompter answering passphrase prompts with a fixed value.
pub struct PassphrasePrompter {
    pub passphrase: String,
    pub asked: AtomicUsize,
}

impl Prompter for PassphrasePrompter {
    fn passphrase<'a>(
        &'a self,
        _key_path: &'a Path,
        _attempt: u32,
    ) -> BoxFuture<'a, Option<Zeroizing<String>>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let p = self.passphrase.clone();
        Box::pin(async move { Some(Zeroizing::new(p)) })
    }
    fn password<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Option<Zeroizing<String>>> {
        Box::pin(async { None })
    }
    fn keyboard_interactive<'a>(
        &'a self,
        _: &'a KbdInteractiveRequest,
    ) -> BoxFuture<'a, Option<Vec<Zeroizing<String>>>> {
        Box::pin(async { None })
    }
}

/// A child process killed on drop (ssh-agent).
pub struct KillOnDrop(pub Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// TCP proxy in front of sshd that can be frozen to simulate a dead peer.
pub struct FreezableProxy {
    pub addr: SocketAddr,
    pub frozen: Arc<AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl FreezableProxy {
    pub async fn start(upstream: SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let frozen = Arc::new(AtomicBool::new(false));
        let f = Arc::clone(&frozen);
        let task = tokio::spawn(async move {
            while let Ok((down, _)) = listener.accept().await {
                let Ok(up) = tokio::net::TcpStream::connect(upstream).await else {
                    continue;
                };
                let (dr, dw) = down.into_split();
                let (ur, uw) = up.into_split();
                tokio::spawn(pump(dr, uw, Arc::clone(&f)));
                tokio::spawn(pump(ur, dw, Arc::clone(&f)));
            }
        });
        Self { addr, frozen, task }
    }

    pub fn freeze(&self) {
        self.frozen.store(true, Ordering::SeqCst);
    }
}

impl Drop for FreezableProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn pump(
    mut r: tokio::net::tcp::OwnedReadHalf,
    mut w: tokio::net::tcp::OwnedWriteHalf,
    frozen: Arc<AtomicBool>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match r.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if frozen.load(Ordering::SeqCst) {
            // Black hole: keep the sockets open but deliver nothing.
            std::future::pending::<()>().await;
        }
        if w.write_all(&buf[..n]).await.is_err() {
            return;
        }
    }
}

/// Run an async test body with the global timeout.
pub async fn within<F: std::future::Future<Output = ()>>(f: F) {
    tokio::time::timeout(TEST_TIMEOUT, f)
        .await
        .expect("test timed out");
}
