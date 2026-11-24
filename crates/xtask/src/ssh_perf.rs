//! SSH measurements against a throwaway local OpenSSH sshd, compared with
//! the stock OpenSSH client where that makes sense.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use termax_ssh::Session;
use termax_sshd_fixture::{Sshd, trusted_ctx};

use crate::metric::Metric;

/// A duplicated tab opens a new channel on the existing connection.
const MAX_NEW_CHANNEL_MS: f64 = 20.0;
/// Channel throughput relative to the OpenSSH client over the same loopback.
const MIN_THROUGHPUT_RATIO: f64 = 0.8;
const BULK_MIB: usize = 128;

pub fn run(metrics: &mut Vec<Metric>) -> Result<()> {
    let Some(sshd) = Sshd::start() else {
        eprintln!("skipping SSH measurements: sshd or ssh-keygen not available");
        return Ok(());
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(measure(&sshd, metrics))
}

async fn measure(sshd: &Sshd, metrics: &mut Vec<Metric>) -> Result<()> {
    let opts = sshd.opts();
    let ctx = trusted_ctx(sshd);

    let mut connects = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let s = Session::connect(&opts, &ctx).await?;
        connects.push(t.elapsed());
        s.disconnect().await?;
    }
    metrics.push(
        Metric::lower(
            "ssh connect (TCP+KEX+auth, loopback)",
            median_ms(&mut connects),
            "ms",
        )
        .ungated(),
    );

    let session = Session::connect(&opts, &ctx).await?;
    let mut execs = Vec::new();
    for _ in 0..20 {
        let t = Instant::now();
        session.exec("true").await?;
        execs.push(t.elapsed());
    }
    metrics.push(
        Metric::lower("ssh new channel + exec `true`", median_ms(&mut execs), "ms")
            .target(MAX_NEW_CHANNEL_MS)
            .ungated(),
    );

    let command = format!("head -c {} /dev/zero", BULK_MIB << 20);
    let t = Instant::now();
    let out = session.exec(&command).await?;
    let ours = t.elapsed();
    if out.stdout.len() != BULK_MIB << 20 {
        bail!("bulk exec returned {} bytes", out.stdout.len());
    }
    session.disconnect().await?;
    let ours_mb_s = mb_per_s(ours);
    metrics
        .push(Metric::higher("ssh channel throughput (termax-ssh)", ours_mb_s, "MB/s").ungated());

    match openssh_bulk(sshd, &command)? {
        Some(theirs) => {
            let theirs_mb_s = mb_per_s(theirs);
            metrics.push(
                Metric::higher(
                    "ssh channel throughput (OpenSSH client)",
                    theirs_mb_s,
                    "MB/s",
                )
                .ungated(),
            );
            metrics.push(
                Metric::higher(
                    "ssh throughput ratio tmx/OpenSSH",
                    ours_mb_s / theirs_mb_s,
                    "x",
                )
                .target(MIN_THROUGHPUT_RATIO)
                .ungated(),
            );
        }
        None => eprintln!("skipping OpenSSH comparison: `ssh` client not available"),
    }
    Ok(())
}

/// Time the stock client for the same command, or `None` if it is missing.
/// The time includes its connection setup; at 128 MiB that is a small share.
fn openssh_bulk(sshd: &Sshd, command: &str) -> Result<Option<Duration>> {
    let known_hosts = sshd.dir.path().join("known_hosts");
    std::fs::write(
        &known_hosts,
        format!(
            "[127.0.0.1]:{} {}",
            sshd.port,
            std::fs::read_to_string(sshd.host_key_path().with_extension("pub"))?
        ),
    )?;
    let t = Instant::now();
    let status = match Command::new("ssh")
        .args([
            "-F",
            "none",
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
        ])
        .arg("-o")
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
        .arg("-i")
        .arg(&sshd.client_key)
        .args(["-p", &sshd.port.to_string()])
        .arg(format!("{}@127.0.0.1", sshd.user))
        .arg(command)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
    {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("running ssh"),
    };
    let elapsed = t.elapsed();
    if !status.success() {
        bail!("OpenSSH client failed: {status}");
    }
    Ok(Some(elapsed))
}

fn mb_per_s(d: Duration) -> f64 {
    (BULK_MIB << 20) as f64 / 1e6 / d.as_secs_f64()
}

fn median_ms(samples: &mut [Duration]) -> f64 {
    samples.sort();
    samples[samples.len() / 2].as_secs_f64() * 1e3
}
