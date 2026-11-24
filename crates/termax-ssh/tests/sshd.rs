//! Integration tests against a real OpenSSH sshd started per test.

mod common;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use common::*;
use termax_ssh::{
    AttemptOutcome, AuthMethod, CloseReason, ConnectContext, Error, ExitStatus, FixedTrust,
    JumpHop, MemoryVerifier, PtyRequest, Session, TrustChoice,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

macro_rules! sshd_or_skip {
    () => {
        match Sshd::start() {
            Some(s) => s,
            None => return,
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn publickey_exec_and_negotiated_algorithms() {
    let sshd = sshd_or_skip!();
    within(async {
        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let out = s.exec("echo hi").await.unwrap();
        assert_eq!(out.stdout, b"hi\n");
        assert!(out.stderr.is_empty());
        assert_eq!(out.exit_status(), Some(0));

        let n = s.negotiated();
        eprintln!("negotiated with {}: {n:#?}", n.server_id);
        assert!(
            n.server_id.starts_with("SSH-2.0-OpenSSH_"),
            "{}",
            n.server_id
        );
        assert!(
            n.strict_kex,
            "strict kex (Terrapin countermeasure) not in effect"
        );
        assert_eq!(n.host_key, "ssh-ed25519");
        assert!(!n.kex.is_empty() && !n.cipher.is_empty());
        assert_eq!(n.post_quantum, termax_ssh::is_post_quantum(&n.kex));
        if openssh_major(&n.server_id) >= 10 {
            // OpenSSH 10 offers mlkem768x25519-sha256 by default.
            assert_eq!(n.kex, "mlkem768x25519-sha256");
            assert!(n.post_quantum);
        }
        assert_eq!(n.cipher, "chacha20-poly1305@openssh.com");
        s.disconnect().await.unwrap();
    })
    .await;
}

fn openssh_major(id: &str) -> u32 {
    id.strip_prefix("SSH-2.0-OpenSSH_")
        .and_then(|v| v.split('.').next())
        .and_then(|m| m.parse().ok())
        .unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn exec_reports_nonzero_exit_and_stderr() {
    let sshd = sshd_or_skip!();
    within(async {
        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let out = s.exec("echo out; echo oops >&2; exit 3").await.unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"oops\n");
        assert_eq!(out.exit, Some(ExitStatus::Code(3)));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pty_shell_reads_term_and_resizes() {
    let sshd = sshd_or_skip!();
    within(async {
        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let mut sh = s
            .open_shell(&PtyRequest {
                term: "xterm-256color".into(),
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
        sh.resize(120, 40).await.unwrap();
        sh.write(b"echo TERM=$TERM; exit\n").await.unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = sh.read().await {
            out.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("TERM=xterm-256color"), "output: {text}");
        assert_eq!(sh.exit_status(), Some(ExitStatus::Code(0)));
        assert!(matches!(
            sh.send_break(250).await,
            Err(Error::Unsupported(_))
        ));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shell_signal_is_delivered_or_ignored_without_error() {
    let sshd = sshd_or_skip!();
    within(async {
        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let sh = s
            .open_shell(&PtyRequest {
                term: "vt100".into(),
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
        sh.signal(termax_ssh::Signal::Int).await.unwrap();
        sh.eof().await.unwrap();
        sh.close().await.unwrap();
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_channels_share_one_connection() {
    let sshd = sshd_or_skip!();
    within(async {
        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let s2 = s.clone();
        let (a, b) = tokio::join!(s.exec("sleep 0.3; echo a"), s2.exec("echo b"));
        assert_eq!(a.unwrap().stdout, b"a\n");
        assert_eq!(b.unwrap().stdout, b"b\n");

        // A shell and an exec at the same time ("Duplicate tab").
        let mut sh = s
            .open_shell(&PtyRequest {
                term: "xterm".into(),
                cols: 80,
                rows: 24,
            })
            .await
            .unwrap();
        let out = s2.exec("echo c").await.unwrap();
        assert_eq!(out.stdout, b"c\n");
        sh.write(b"exit 0\n").await.unwrap();
        while sh.read().await.is_some() {}
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_host_key_reject_accept_once_and_save() {
    let sshd = sshd_or_skip!();
    within(async {
        let verifier = Arc::new(MemoryVerifier::new());
        let hp = sshd.host_port();

        // Reject aborts.
        let trust = CountingTrust::new(TrustChoice::Reject);
        let err = Session::connect(&sshd.opts(), &ctx(verifier.clone(), trust.clone()))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::HostKeyRejected { host_port, .. } if *host_port == hp),
            "{err}"
        );
        assert_eq!(trust.count(), 1);

        // Accept once connects but stores nothing.
        let trust = CountingTrust::new(TrustChoice::AcceptOnce);
        let s = Session::connect(&sshd.opts(), &ctx(verifier.clone(), trust.clone()))
            .await
            .unwrap();
        assert_eq!(s.exec("true").await.unwrap().exit_status(), Some(0));
        assert_eq!(verifier.get(&hp, "ssh-ed25519"), None);

        // Accept and save stores the OpenSSH-format fingerprint.
        let trust = CountingTrust::new(TrustChoice::AcceptAndSave);
        Session::connect(&sshd.opts(), &ctx(verifier.clone(), trust.clone()))
            .await
            .unwrap();
        let saved = verifier.get(&hp, "ssh-ed25519").expect("saved");
        assert_eq!(saved, sshd.host_fingerprint());
        let line = format!("127.0.0.1 {}", pubkey_line(&sshd.host_key_path()));
        let kh = termax_interop::KnownHosts::parse_str(&line);
        assert_eq!(saved, kh.entries[0].fingerprint_sha256());

        // Reconnect: trusted, no prompt.
        let trust = CountingTrust::new(TrustChoice::Reject);
        Session::connect(&sshd.opts(), &ctx(verifier.clone(), trust.clone()))
            .await
            .unwrap();
        assert_eq!(trust.count(), 0);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_host_key_always_aborts() {
    let mut sshd = sshd_or_skip!();
    let verifier = sshd.trusting_verifier();
    let old_fp = sshd.host_fingerprint();
    sshd.rotate_host_key();
    within(async {
        // Even a trust callback that would accept anything is never asked.
        let trust = CountingTrust::new(TrustChoice::AcceptAndSave);
        let err = Session::connect(&sshd.opts(), &ctx(verifier.clone(), trust.clone()))
            .await
            .unwrap_err();
        match err {
            Error::HostKeyChanged {
                old_fingerprint,
                new_fingerprint,
                ..
            } => {
                assert_eq!(old_fingerprint, old_fp);
                assert_eq!(new_fingerprint, sshd.host_fingerprint());
            }
            other => panic!("expected HostKeyChanged, got {other}"),
        }
        assert_eq!(trust.count(), 0);
        assert_eq!(
            verifier.get(&sshd.host_port(), "ssh-ed25519").as_deref(),
            Some(old_fp.as_str())
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoked_host_key_aborts() {
    let sshd = sshd_or_skip!();
    within(async {
        let verifier = sshd.trusting_verifier();
        verifier.revoke(&sshd.host_fingerprint());
        let err = Session::connect(
            &sshd.opts(),
            &ctx(verifier, Arc::new(FixedTrust(TrustChoice::AcceptOnce))),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::HostKeyRevoked { .. }), "{err}");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn local_forward_round_trips_and_stops() {
    let sshd = sshd_or_skip!();
    within(async {
        // Echo server the forward targets (reached from the sshd side).
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_port = echo.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut c, _)) = echo.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = c.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });

        let s = Session::connect(&sshd.opts(), &trusted_ctx(&sshd))
            .await
            .unwrap();
        let fwd = s
            .forward_local("127.0.0.1:0".parse().unwrap(), "127.0.0.1", echo_port)
            .await
            .unwrap();
        let local = fwd.local_addr();

        for _ in 0..2 {
            let mut c = tokio::net::TcpStream::connect(local).await.unwrap();
            let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
            c.write_all(&payload).await.unwrap();
            let mut back = vec![0u8; payload.len()];
            c.read_exact(&mut back).await.unwrap();
            assert_eq!(back, payload);
        }

        // Binding an address that is in use is a typed error.
        let err = s
            .forward_local(local, "127.0.0.1", echo_port)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::ForwardBind { .. }), "{err}");

        fwd.close().await;
        assert!(tokio::net::TcpStream::connect(local).await.is_err());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_jump_verifies_each_hop() {
    let sshd = sshd_or_skip!();
    within(async {
        let rec = Arc::new(RecordingVerifier {
            inner: sshd.trusting_verifier(),
            seen: Default::default(),
        });
        let mut opts = sshd.opts();
        opts.jump_hosts = vec![
            JumpHop {
                host: "127.0.0.1".into(),
                port: sshd.port,
                user: sshd.user.clone(),
                auth: vec![key_auth(&sshd.client_key)],
            },
            JumpHop {
                host: "127.0.0.1".into(),
                port: sshd.port,
                user: sshd.user.clone(),
                auth: vec![key_auth(&sshd.client_key)],
            },
        ];
        let s = Session::connect(
            &opts,
            &ctx(rec.clone(), Arc::new(FixedTrust(TrustChoice::Reject))),
        )
        .await
        .unwrap();
        assert_eq!(s.exec("echo via").await.unwrap().stdout, b"via\n");
        let seen = rec.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3, "each hop's host key must be verified");
        assert!(seen.iter().all(|k| k.host_port == sshd.host_port()));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn proxy_jump_with_untrusted_hop_aborts() {
    let sshd = sshd_or_skip!();
    within(async {
        let mut opts = sshd.opts();
        opts.jump_hosts = vec![JumpHop {
            host: "127.0.0.1".into(),
            port: sshd.port,
            user: sshd.user.clone(),
            auth: vec![key_auth(&sshd.client_key)],
        }];
        let trust = CountingTrust::new(TrustChoice::Reject);
        let err = Session::connect(&opts, &ctx(Arc::new(MemoryVerifier::new()), trust.clone()))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::HostKeyRejected { .. }), "{err}");
        assert_eq!(trust.count(), 1, "aborted at the first hop");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn agent_auth() {
    let sshd = sshd_or_skip!();
    if !have("ssh-agent") || !have("ssh-add") {
        eprintln!("SKIP: ssh-agent/ssh-add not available");
        return;
    }
    let agent_dir = tempfile::Builder::new().prefix("oxa").tempdir().unwrap();
    let sock = agent_dir.path().join("agent.sock");
    let _agent = KillOnDrop(
        std::process::Command::new("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&sock)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = std::process::Command::new("ssh-add")
        .arg(&sshd.client_key)
        .env("SSH_AUTH_SOCK", &sock)
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "ssh-add failed");

    within(async {
        let mut opts = sshd.opts();
        opts.auth = vec![AuthMethod::Agent {
            socket: Some(sock.clone()),
        }];
        let s = Session::connect(&opts, &trusted_ctx(&sshd)).await.unwrap();
        assert_eq!(s.exec("echo agent").await.unwrap().stdout, b"agent\n");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_key_lists_attempted_methods() {
    let sshd = sshd_or_skip!();
    let other = sshd.dir.path().join("id_other");
    keygen(&other, "");
    within(async {
        let mut opts = sshd.opts();
        opts.auth = vec![
            key_auth(&other),
            AuthMethod::PublicKeyFile {
                path: sshd.dir.path().join("missing"),
                passphrase: None,
            },
            AuthMethod::Password(None),
            AuthMethod::KeyboardInteractive,
        ];
        let err = Session::connect(&opts, &trusted_ctx(&sshd))
            .await
            .unwrap_err();
        let msg = err.to_string();
        let Error::AuthFailed {
            user,
            attempts,
            server_methods,
        } = err
        else {
            panic!("expected AuthFailed, got {msg}");
        };
        assert_eq!(user, sshd.user);
        assert_eq!(server_methods, vec!["publickey".to_owned()]);
        let outcomes: Vec<_> = attempts.iter().map(|a| (a.method, &a.outcome)).collect();
        assert_eq!(outcomes[0], ("publickey", &AttemptOutcome::Rejected));
        assert!(matches!(
            outcomes[1],
            ("publickey", AttemptOutcome::Unavailable(_))
        ));
        assert_eq!(outcomes[2], ("password", &AttemptOutcome::NotOffered));
        assert_eq!(
            outcomes[3],
            ("keyboard-interactive", &AttemptOutcome::NotOffered)
        );
        assert!(
            msg.contains("id_other") && msg.contains("rejected"),
            "{msg}"
        );
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn encrypted_key_uses_passphrase_prompt() {
    let sshd = sshd_or_skip!();
    let key = sshd.dir.path().join("id_locked");
    keygen(&key, "correct horse");
    sshd.authorize(&key);
    within(async {
        let mut opts = sshd.opts();
        opts.auth = vec![key_auth(&key)];
        let prompter = Arc::new(PassphrasePrompter {
            passphrase: "correct horse".into(),
            asked: AtomicUsize::new(0),
        });
        let c = ConnectContext {
            prompter: prompter.clone(),
            ..trusted_ctx(&sshd)
        };
        let s = Session::connect(&opts, &c).await.unwrap();
        assert_eq!(s.exec("echo ok").await.unwrap().stdout, b"ok\n");
        assert_eq!(prompter.asked.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Cancelling the prompt skips the key.
        let err = Session::connect(&opts, &trusted_ctx(&sshd))
            .await
            .unwrap_err();
        match err {
            Error::AuthFailed { attempts, .. } => {
                assert_eq!(attempts[0].outcome, AttemptOutcome::Skipped);
            }
            other => panic!("expected AuthFailed, got {other}"),
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keepalive_misses_mark_connection_dead() {
    let sshd = sshd_or_skip!();
    within(async {
        let proxy = FreezableProxy::start(sshd.addr()).await;
        let mut opts = sshd.opts();
        opts.host = "127.0.0.1".into();
        opts.port = proxy.addr.port();
        opts.keepalive_interval = Some(Duration::from_millis(300));
        opts.keepalive_max_missed = 2;
        // The proxy port differs, so trust the key under the proxy's host:port.
        let verifier = Arc::new(MemoryVerifier::new());
        verifier.insert(
            &format!("127.0.0.1:{}", proxy.addr.port()),
            "ssh-ed25519",
            &sshd.host_fingerprint(),
        );
        let s = Session::connect(
            &opts,
            &ctx(verifier, Arc::new(FixedTrust(TrustChoice::Reject))),
        )
        .await
        .unwrap();
        // Healthy while the server answers.
        tokio::time::sleep(Duration::from_millis(900)).await;
        assert_eq!(s.state(), termax_ssh::ConnectionState::Connected);

        proxy.freeze();
        let reason = tokio::time::timeout(Duration::from_secs(10), s.closed())
            .await
            .expect("keepalive timeout not detected");
        assert_eq!(reason, CloseReason::KeepaliveTimeout);
        assert!(s.is_closed());
        assert!(matches!(s.exec("true").await, Err(Error::Closed(_))));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn connect_timeout_is_enforced() {
    // A listener that accepts but never speaks SSH.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let _hold = tokio::spawn(async move {
        let mut socks = Vec::new();
        while let Ok((s, _)) = l.accept().await {
            socks.push(s);
        }
    });
    within(async {
        let mut opts = termax_ssh::ConnectOptions::new("127.0.0.1", "nobody");
        opts.port = port;
        opts.connect_timeout = Duration::from_millis(500);
        let err = Session::connect(
            &opts,
            &ctx(
                Arc::new(MemoryVerifier::new()),
                Arc::new(FixedTrust(TrustChoice::Reject)),
            ),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Timeout { .. }), "{err}");
    })
    .await;
}
