//! A loopback TCP proxy in front of the fixture sshd that simulates network
//! faults: added latency, a network that goes down and comes back, and
//! connections dropped mid-session. Netem needs root; this does not.
//!
//! Plain threads, so it works from sync and async tests alike.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Default)]
struct State {
    stop: AtomicBool,
    down: AtomicBool,
    delay_us: AtomicU64,
    accepted: AtomicUsize,
    /// Both sockets of every live connection, so they can be severed.
    conns: Mutex<Vec<TcpStream>>,
}

impl State {
    fn sever_all(&self) {
        let conns = std::mem::take(&mut *self.conns.lock().unwrap_or_else(|p| p.into_inner()));
        for c in conns {
            let _ = c.shutdown(Shutdown::Both);
        }
    }
}

/// TCP proxy on `127.0.0.1:<random port>` forwarding to `upstream`.
pub struct NetProxy {
    addr: SocketAddr,
    state: Arc<State>,
    accept: Option<JoinHandle<()>>,
}

impl NetProxy {
    /// Start forwarding to `upstream`.
    pub fn start(upstream: SocketAddr) -> NetProxy {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind proxy");
        listener.set_nonblocking(true).expect("nonblocking proxy");
        let addr = listener.local_addr().expect("proxy addr");
        let state = Arc::new(State::default());
        let st = Arc::clone(&state);
        let accept = thread::Builder::new()
            .name("tmx-test-proxy".into())
            .spawn(move || accept_loop(&listener, upstream, &st))
            .expect("spawn proxy");
        NetProxy {
            addr,
            state,
            accept: Some(accept),
        }
    }

    /// Address clients connect to.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Port clients connect to.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Connections forwarded to the upstream so far.
    pub fn accepted(&self) -> usize {
        self.state.accepted.load(Ordering::SeqCst)
    }

    /// Delay every forwarded chunk, in both directions, by `delay`.
    pub fn set_delay(&self, delay: Duration) {
        let us = u64::try_from(delay.as_micros()).unwrap_or(u64::MAX);
        self.state.delay_us.store(us, Ordering::SeqCst);
    }

    /// The network goes down: data on existing connections is silently
    /// dropped (a black hole, so only keepalives can notice) and new
    /// connections are closed as soon as they are accepted.
    pub fn go_down(&self) {
        self.state.down.store(true, Ordering::SeqCst);
    }

    /// The network comes back. Connections that lived through the outage
    /// are closed, as the server has long given up on them.
    pub fn come_up(&self) {
        self.state.sever_all();
        self.state.down.store(false, Ordering::SeqCst);
    }

    /// Close every current connection (both directions), as when a NAT or
    /// firewall drops its state. New connections are still accepted.
    pub fn sever(&self) {
        self.state.sever_all();
    }
}

impl Drop for NetProxy {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::SeqCst);
        self.state.sever_all();
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(listener: &TcpListener, upstream: SocketAddr, state: &Arc<State>) {
    while !state.stop.load(Ordering::SeqCst) {
        let down = match listener.accept() {
            Ok((down, _)) => down,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(_) => return,
        };
        if state.down.load(Ordering::SeqCst) {
            let _ = down.shutdown(Shutdown::Both);
            continue;
        }
        let _ = down.set_nonblocking(false);
        let Ok(up) = TcpStream::connect(upstream) else {
            continue;
        };
        let _ = down.set_nodelay(true);
        let _ = up.set_nodelay(true);
        let clones = (
            down.try_clone(),
            up.try_clone(),
            down.try_clone(),
            up.try_clone(),
        );
        let (Ok(d1), Ok(u1), Ok(d2), Ok(u2)) = clones else {
            continue;
        };
        {
            let mut conns = state.conns.lock().unwrap_or_else(|p| p.into_inner());
            conns.push(d2);
            conns.push(u2);
        }
        state.accepted.fetch_add(1, Ordering::SeqCst);
        let (s1, s2) = (Arc::clone(state), Arc::clone(state));
        thread::spawn(move || pump(down, u1, &s1));
        thread::spawn(move || pump(up, d1, &s2));
    }
}

fn pump(mut from: TcpStream, mut to: TcpStream, state: &State) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if state.down.load(Ordering::SeqCst) {
            // Black hole: the bytes are lost.
            continue;
        }
        let delay = state.delay_us.load(Ordering::SeqCst);
        if delay > 0 {
            thread::sleep(Duration::from_micros(delay));
        }
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn echo_server() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                thread::spawn(move || {
                    let mut r = s.try_clone().unwrap();
                    let mut w = s;
                    let _ = std::io::copy(&mut r, &mut w);
                });
            }
        });
        addr
    }

    fn round_trip(addr: SocketAddr) -> std::io::Result<Vec<u8>> {
        let mut s = TcpStream::connect(addr)?;
        s.set_read_timeout(Some(Duration::from_millis(500)))?;
        s.write_all(b"ping")?;
        let mut buf = [0u8; 4];
        s.read_exact(&mut buf)?;
        Ok(buf.to_vec())
    }

    #[test]
    fn forwards_delays_goes_down_and_comes_back() {
        let proxy = NetProxy::start(echo_server());
        assert_eq!(round_trip(proxy.addr()).unwrap(), b"ping");
        assert_eq!(proxy.accepted(), 1);

        proxy.set_delay(Duration::from_millis(50));
        let t = Instant::now();
        assert_eq!(round_trip(proxy.addr()).unwrap(), b"ping");
        assert!(
            t.elapsed() >= Duration::from_millis(100),
            "both directions delayed"
        );
        proxy.set_delay(Duration::ZERO);

        // A live connection black-holes while the network is down.
        let mut live = TcpStream::connect(proxy.addr()).unwrap();
        live.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        thread::sleep(Duration::from_millis(50));
        proxy.go_down();
        live.write_all(b"lost").unwrap();
        let mut buf = [0u8; 4];
        assert!(live.read_exact(&mut buf).is_err(), "nothing delivered");
        assert!(round_trip(proxy.addr()).is_err(), "new connections fail");

        proxy.come_up();
        let mut tail = Vec::new();
        live.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(live.read_to_end(&mut tail).unwrap_or(0), 0, "severed");
        assert_eq!(round_trip(proxy.addr()).unwrap(), b"ping");
    }
}
