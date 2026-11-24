//! `cargo xtask sshd` — run the test fixture's throwaway OpenSSH sshd in the
//! foreground so it can be used by hand, and stop it cleanly on Ctrl-C.

use anyhow::{Context, Result, bail};
use termax_sshd_fixture::Sshd;

pub fn run() -> Result<()> {
    let Some(sshd) = Sshd::start() else {
        bail!("sshd or ssh-keygen is not available on this machine");
    };
    let known_hosts = sshd.dir.path().join("known_hosts");
    let host_pub = std::fs::read_to_string(sshd.host_key_path().with_extension("pub"))?;
    std::fs::write(
        &known_hosts,
        format!("[127.0.0.1]:{} {host_pub}", sshd.port),
    )?;

    println!("Throwaway sshd listening on 127.0.0.1:{}", sshd.port);
    println!("  user:             {}", sshd.user);
    println!("  client key:       {}", sshd.client_key.display());
    println!("  host fingerprint: {}", sshd.host_fingerprint());
    println!("  known_hosts:      {}", known_hosts.display());
    println!();
    println!(
        "Connect: ssh -F none -i {} -o IdentitiesOnly=yes -o UserKnownHostsFile={} -p {} {}@127.0.0.1",
        sshd.client_key.display(),
        known_hosts.display(),
        sshd.port,
        sshd.user
    );
    println!("Press Ctrl-C to stop; the temporary directory is removed on exit.");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(stop_signal())
        .context("waiting for a stop signal")?;
    // Dropping the fixture kills sshd and removes its temporary key directory.
    drop(sshd);
    println!("\nsshd stopped.");
    Ok(())
}

/// Wait for Ctrl-C, or on Unix also SIGTERM (e.g. `timeout`, process
/// managers), so cleanup runs either way.
async fn stop_signal() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r,
            _ = term.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await
    }
}
