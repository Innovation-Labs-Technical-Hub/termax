//! Test harness: a throwaway OpenSSH sshd running as the current user.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use oxt_ssh::{
    AuthMethod, BoxFuture, ConnectContext, ConnectOptions, FixedTrust, HostKeyInfo, HostKeyStatus,
    HostKeyVerifier, KbdInteractiveRequest, MemoryVerifier, NoPrompter, Prompter, TrustChoice,
    TrustDecision, Zeroizing,
};
use tempfile::TempDir;

pub const SSHD: &str = "/usr/sbin/sshd";

/// Upper bound for any single test body.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(60);

pub fn have(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub fn current_user() -> String {
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| {
            let out = Command::new("id").arg("-un").output().expect("id -un");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        })
}

pub fn keygen(path: &Path, passphrase: &str) {
    let st = Command::new("ssh-keygen")
        .args([
            "-q", "-t", "ed25519", "-N", passphrase, "-C", "oxt-test", "-f",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .expect("ssh-keygen");
    assert!(st.success(), "ssh-keygen failed");
}

pub fn pubkey_line(priv_path: &Path) -> String {
    std::fs::read_to_string(priv_path.with_extension("pub")).expect("read .pub")
}

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("bind :0")
        .port()
}

pub struct Sshd {
    pub dir: TempDir,
    pub port: u16,
    pub user: String,
    /// Authorized client key (no passphrase).
    pub client_key: PathBuf,
    child: Option<Child>,
}

impl Sshd {
    /// Start sshd, or `None` (after printing a skip message) if it is missing.
    pub fn start() -> Option<Sshd> {
        if !Path::new(SSHD).exists() || !have("ssh-keygen") {
            eprintln!("SKIP: {SSHD} or ssh-keygen not available");
            return None;
        }
        let dir = tempfile::Builder::new()
            .prefix("oxt")
            .tempdir()
            .expect("tempdir");
        let host_key = dir.path().join("host_key");
        keygen(&host_key, "");
        let client_key = dir.path().join("id_client");
        keygen(&client_key, "");
        std::fs::write(dir.path().join("authorized_keys"), pubkey_line(&client_key))
            .expect("authorized_keys");
        let mut s = Sshd {
            dir,
            port: 0,
            user: current_user(),
            client_key,
            child: None,
        };
        s.spawn_with_retries();
        Some(s)
    }

    pub fn host_key_path(&self) -> PathBuf {
        self.dir.path().join("host_key")
    }

    /// Authorize another public key.
    pub fn authorize(&self, priv_path: &Path) {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(self.dir.path().join("authorized_keys"))
            .expect("open authorized_keys");
        f.write_all(pubkey_line(priv_path).as_bytes())
            .expect("append");
    }

    fn write_config(&self) -> PathBuf {
        let d = self.dir.path().display();
        let cfg = format!(
            "Port {port}\n\
             ListenAddress 127.0.0.1\n\
             HostKey {d}/host_key\n\
             AuthorizedKeysFile {d}/authorized_keys\n\
             PidFile {d}/sshd.pid\n\
             StrictModes no\n\
             UsePAM no\n\
             PasswordAuthentication no\n\
             KbdInteractiveAuthentication no\n\
             AllowTcpForwarding yes\n\
             PerSourcePenalties no\n\
             LogLevel VERBOSE\n",
            port = self.port
        );
        let path = self.dir.path().join("sshd_config");
        std::fs::write(&path, cfg).expect("sshd_config");
        path
    }

    fn spawn_with_retries(&mut self) {
        for _ in 0..5 {
            if self.port == 0 {
                self.port = free_port();
            }
            if self.try_spawn() {
                return;
            }
            self.port = 0;
        }
        panic!("sshd did not start; see {}", self.log_path().display());
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("sshd.log")
    }

    fn try_spawn(&mut self) -> bool {
        let cfg = self.write_config();
        let log = std::fs::File::create(self.log_path()).expect("log file");
        let child = Command::new(SSHD)
            .args(["-D", "-e", "-f"])
            .arg(&cfg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("spawn sshd");
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(Ok(Some(_))) = self.child.as_mut().map(Child::try_wait) {
                self.child = None;
                return false;
            }
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", self.port)) {
                // Make sure it is sshd answering, not some other listener.
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 4];
                if s.read_exact(&mut buf).is_ok() && &buf == b"SSH-" {
                    return true;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.kill();
        false
    }

    fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Replace the host key and restart sshd on the same port.
    pub fn rotate_host_key(&mut self) {
        self.kill();
        let hk = self.host_key_path();
        let _ = std::fs::remove_file(&hk);
        let _ = std::fs::remove_file(hk.with_extension("pub"));
        keygen(&hk, "");
        let port = self.port;
        for _ in 0..40 {
            if self.try_spawn() {
                return;
            }
            self.port = port;
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("sshd restart failed");
    }

    /// Fingerprint of the current host key according to `ssh-keygen -l`.
    pub fn host_fingerprint(&self) -> String {
        let out = Command::new("ssh-keygen")
            .args(["-l", "-E", "sha256", "-f"])
            .arg(self.host_key_path().with_extension("pub"))
            .output()
            .expect("ssh-keygen -l");
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .nth(1)
            .expect("fingerprint field")
            .to_owned()
    }

    pub fn host_port(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    /// Options for the authorized client key with quick timeouts.
    pub fn opts(&self) -> ConnectOptions {
        let mut o = ConnectOptions::new("127.0.0.1", &self.user);
        o.port = self.port;
        o.connect_timeout = Duration::from_secs(20);
        o.auth = vec![key_auth(&self.client_key)];
        o
    }

    /// A verifier that already trusts this server's key.
    pub fn trusting_verifier(&self) -> Arc<MemoryVerifier> {
        let v = Arc::new(MemoryVerifier::new());
        v.insert(&self.host_port(), "ssh-ed25519", &self.host_fingerprint());
        v
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        self.kill();
    }
}

pub fn key_auth(path: &Path) -> AuthMethod {
    AuthMethod::PublicKeyFile {
        path: path.to_owned(),
        passphrase: None,
    }
}

pub fn ctx(verifier: Arc<dyn HostKeyVerifier>, trust: Arc<dyn TrustDecision>) -> ConnectContext {
    ConnectContext {
        verifier,
        trust,
        prompter: Arc::new(NoPrompter),
    }
}

pub fn trusted_ctx(sshd: &Sshd) -> ConnectContext {
    ctx(
        sshd.trusting_verifier(),
        Arc::new(FixedTrust(TrustChoice::Reject)),
    )
}

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
    fn verify<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, oxt_ssh::Result<HostKeyStatus>> {
        self.seen.lock().unwrap().push(key.clone());
        self.inner.verify(key)
    }
    fn save<'a>(&'a self, key: &'a HostKeyInfo) -> BoxFuture<'a, oxt_ssh::Result<()>> {
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
