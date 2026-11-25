//! Name resolution and TCP connection establishment: async, cancellable DNS
//! with a timeout, and Happy Eyeballs v2 (RFC 8305) across the resolved
//! addresses.
//!
//! RFC 8305 is implemented minimally: addresses are interleaved by family
//! starting with the family of the first answer, and a new attempt starts
//! every Connection Attempt Delay (250 ms) or as soon as the previous attempt
//! fails; the first connection to succeed wins and the others are dropped.
//! There is no separate AAAA/A query race, because the system resolver
//! returns both families in one answer.

use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpSocket, TcpStream};
use tokio::task::JoinSet;

use crate::clock::{BoxFuture, Clock};
use crate::codes::{Disconnect, ErrorCode};

/// Default DNS timeout.
pub const DEFAULT_DNS_TIMEOUT: Duration = Duration::from_secs(5);
/// RFC 8305 recommended Connection Attempt Delay.
pub const DEFAULT_ATTEMPT_DELAY: Duration = Duration::from_millis(250);
/// Socket receive buffer requested for SSH connections.
pub const DEFAULT_RECV_BUFFER: u32 = 256 * 1024;

/// Turns a host name into socket addresses.
pub trait Resolver: Send + Sync + 'static {
    /// Shown in diagnostics, e.g. `system (getaddrinfo)`.
    fn name(&self) -> String;

    /// Resolve `host`. Dropping the future cancels the lookup as far as the
    /// caller is concerned.
    fn lookup(&self, host: &str, port: u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>>;
}

/// The operating system resolver (`getaddrinfo` on tokio's blocking pool,
/// never on the caller's thread).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemResolver;

impl Resolver for SystemResolver {
    fn name(&self) -> String {
        "system (getaddrinfo)".into()
    }

    fn lookup(&self, host: &str, port: u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>> {
        if let Ok(ip) = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
        {
            return Box::pin(async move { Ok(vec![SocketAddr::new(ip, port)]) });
        }
        let host = host.to_owned();
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host.as_str(), port))
                .await?
                .collect())
        })
    }
}

/// Resolve `host`, failing with `TMX-DNS-001` after `timeout` and
/// `TMX-DNS-002` when the name does not resolve. Diagnostics name the
/// resolver and the records received.
pub async fn resolve(
    resolver: &dyn Resolver,
    clock: &dyn Clock,
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<Vec<SocketAddr>, Disconnect> {
    let lookup = resolver.lookup(host, port);
    let result = tokio::select! {
        r = lookup => r,
        () = clock.sleep(timeout) => {
            return Err(Disconnect::new(
                ErrorCode::DnsTimeout,
                format!("resolver {} did not answer for {host} within {timeout:?}; records received: none", resolver.name()),
            ));
        }
    };
    match result {
        Ok(addrs) if !addrs.is_empty() => Ok(addrs),
        Ok(_) => Err(Disconnect::new(
            ErrorCode::DnsNotFound,
            format!(
                "resolver {} returned no addresses for {host}; records received: none",
                resolver.name()
            ),
        )),
        Err(e) => Err(Disconnect::new(
            ErrorCode::DnsNotFound,
            format!(
                "resolver {} could not resolve {host}: {e}; records received: none",
                resolver.name()
            ),
        )),
    }
}

/// Order addresses for Happy Eyeballs: alternate families, starting with the
/// family of the first address, keeping the resolver's order within each.
pub fn interleave(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return Vec::new();
    };
    let (mut primary, mut secondary): (Vec<SocketAddr>, Vec<SocketAddr>) =
        addrs.iter().partition(|a| a.is_ipv6() == first.is_ipv6());
    primary.reverse();
    secondary.reverse();
    let mut out = Vec::with_capacity(addrs.len());
    while !primary.is_empty() || !secondary.is_empty() {
        out.extend(primary.pop());
        out.extend(secondary.pop());
    }
    out
}

/// Race connections to `addrs` (RFC 8305 §5): start one attempt, then the
/// next after `attempt_delay` or as soon as one fails. Returns the first
/// connection that succeeds and its address, or the last error.
pub async fn happy_eyeballs<T, F, Fut>(
    addrs: &[SocketAddr],
    clock: &dyn Clock,
    attempt_delay: Duration,
    connect: F,
) -> io::Result<(T, SocketAddr)>
where
    T: Send + 'static,
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>> + Send + 'static,
{
    let order = interleave(addrs);
    let mut pending = order.into_iter();
    let mut set = JoinSet::new();
    let mut last_err = io::Error::new(io::ErrorKind::InvalidInput, "no addresses to connect to");
    let start = |set: &mut JoinSet<_>, pending: &mut std::vec::IntoIter<SocketAddr>| {
        if let Some(addr) = pending.next() {
            let fut = connect(addr);
            set.spawn(async move { (addr, fut.await) });
            true
        } else {
            false
        }
    };
    if !start(&mut set, &mut pending) {
        return Err(last_err);
    }
    loop {
        let more = pending.len() > 0;
        tokio::select! {
            joined = set.join_next() => match joined {
                Some(Ok((addr, Ok(stream)))) => {
                    set.abort_all();
                    return Ok((stream, addr));
                }
                Some(Ok((addr, Err(e)))) => {
                    tracing::debug!(%addr, error = %e, "connection attempt failed");
                    last_err = e;
                    start(&mut set, &mut pending);
                }
                Some(Err(join)) => {
                    last_err = io::Error::other(join.to_string());
                    start(&mut set, &mut pending);
                }
                None => return Err(last_err),
            },
            () = clock.sleep(attempt_delay), if more => {
                start(&mut set, &mut pending);
            }
        }
    }
}

/// One TCP attempt with the socket options SSH wants: `SO_KEEPALIVE`,
/// `TCP_NODELAY` and a larger receive buffer.
pub async fn tcp_connect(addr: SocketAddr, recv_buffer: u32) -> io::Result<TcpStream> {
    let socket = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_keepalive(true)?;
    if let Err(e) = socket.set_recv_buffer_size(recv_buffer) {
        tracing::debug!(error = %e, "could not set the socket receive buffer");
    }
    let stream = socket.connect(addr).await?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// Resolve and connect: DNS with `dns_timeout`, then Happy Eyeballs bounded
/// by `connect_timeout`.
pub(crate) struct Establish<'a> {
    pub resolver: &'a dyn Resolver,
    pub clock: &'a Arc<dyn Clock>,
    pub host: &'a str,
    pub port: u16,
    pub dns_timeout: Duration,
    pub connect_timeout: Duration,
    pub attempt_delay: Duration,
    pub recv_buffer: u32,
}

impl Establish<'_> {
    /// Resolve the name (the caller moves to `Connecting` afterwards).
    pub async fn resolve(&self) -> Result<Vec<SocketAddr>, Disconnect> {
        resolve(
            self.resolver,
            self.clock.as_ref(),
            self.host,
            self.port,
            self.dns_timeout,
        )
        .await
    }

    /// Connect to one of `addrs`.
    pub async fn connect(&self, addrs: &[SocketAddr]) -> Result<TcpStream, Disconnect> {
        let recv = self.recv_buffer;
        let race = happy_eyeballs(addrs, self.clock.as_ref(), self.attempt_delay, move |a| {
            tcp_connect(a, recv)
        });
        let target = format!("{}:{}", self.host, self.port);
        tokio::select! {
            r = race => r.map(|(s, addr)| {
                tracing::debug!(%target, %addr, "tcp connected");
                s
            }).map_err(|e| Disconnect::new(crate::codes::code_for_io(&e), format!("{target}: {e}"))),
            () = self.clock.sleep(self.connect_timeout) => Err(Disconnect::new(
                ErrorCode::NetTimeout,
                format!("{target}: no TCP connection within {:?}", self.connect_timeout),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use std::sync::Mutex;

    fn v4(n: u8) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, n], 22))
    }
    fn v6(n: u16) -> SocketAddr {
        SocketAddr::from(([0x2001, 0xdb8, 0, 0, 0, 0, 0, n], 22))
    }

    #[test]
    fn interleaves_families_starting_with_the_first() {
        assert_eq!(
            interleave(&[v6(1), v6(2), v4(1), v4(2), v4(3)]),
            [v6(1), v4(1), v6(2), v4(2), v4(3)]
        );
        assert_eq!(interleave(&[v4(1), v6(1)]), [v4(1), v6(1)]);
        assert!(interleave(&[]).is_empty());
    }

    struct Hang;
    impl Resolver for Hang {
        fn name(&self) -> String {
            "black hole".into()
        }
        fn lookup(&self, _: &str, _: u16) -> BoxFuture<'static, io::Result<Vec<SocketAddr>>> {
            Box::pin(std::future::pending())
        }
    }

    #[tokio::test]
    async fn dns_times_out_with_the_resolver_named() {
        let clock = Arc::new(ManualClock::new());
        let c = Arc::clone(&clock);
        let task = tokio::spawn(async move {
            resolve(
                &Hang,
                c.as_ref(),
                "example.invalid",
                22,
                DEFAULT_DNS_TIMEOUT,
            )
            .await
        });
        while clock.sleepers() == 0 {
            tokio::task::yield_now().await;
        }
        clock.advance(DEFAULT_DNS_TIMEOUT);
        let err = task.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::DnsTimeout);
        assert!(err.detail.contains("black hole"), "{}", err.detail);
    }

    #[tokio::test]
    async fn ip_literals_skip_the_resolver() {
        let addrs = SystemResolver.lookup("[::1]", 22).await.unwrap();
        assert_eq!(addrs, [SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 1], 22))]);
    }

    /// An unreachable IPv6 address that hangs and a good IPv4 one: the IPv4
    /// attempt starts after the attempt delay and wins.
    #[tokio::test]
    async fn happy_eyeballs_starts_the_next_family_after_the_attempt_delay() {
        let clock = Arc::new(ManualClock::new());
        let started: Arc<Mutex<Vec<(SocketAddr, Duration)>>> = Arc::default();
        let (c, s) = (Arc::clone(&clock), Arc::clone(&started));
        let t0 = clock.now();
        let race = tokio::spawn(async move {
            let c2 = Arc::clone(&c);
            happy_eyeballs(
                &[v6(1), v4(1)],
                c.as_ref(),
                DEFAULT_ATTEMPT_DELAY,
                move |a| {
                    s.lock().unwrap().push((a, c2.now() - t0));
                    async move {
                        if a.is_ipv6() {
                            std::future::pending::<io::Result<SocketAddr>>().await
                        } else {
                            Ok(a)
                        }
                    }
                },
            )
            .await
        });
        while clock.sleepers() == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(started.lock().unwrap().len(), 1, "only IPv6 so far");
        clock.advance(DEFAULT_ATTEMPT_DELAY);
        let (got, addr) = race.await.unwrap().unwrap();
        assert_eq!((got, addr), (v4(1), v4(1)));
        let started = started.lock().unwrap();
        assert_eq!(started[0], (v6(1), Duration::ZERO));
        assert_eq!(started[1], (v4(1), DEFAULT_ATTEMPT_DELAY));
    }

    #[tokio::test]
    async fn a_failed_attempt_starts_the_next_at_once_and_errors_propagate() {
        let clock = ManualClock::new();
        let r = happy_eyeballs(
            &[v6(1), v4(1)],
            &clock,
            DEFAULT_ATTEMPT_DELAY,
            |a| async move {
                if a.is_ipv6() {
                    Err(io::Error::from(io::ErrorKind::NetworkUnreachable))
                } else {
                    Ok(a)
                }
            },
        )
        .await;
        assert_eq!(
            r.unwrap().1,
            v4(1),
            "no attempt delay needed after a failure"
        );

        let all_fail = happy_eyeballs(&[v4(1), v4(2)], &clock, DEFAULT_ATTEMPT_DELAY, |_| async {
            Err::<(), _>(io::Error::from(io::ErrorKind::ConnectionRefused))
        })
        .await;
        assert_eq!(
            all_fail.unwrap_err().kind(),
            io::ErrorKind::ConnectionRefused
        );
    }
}
