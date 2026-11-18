//! Servers on the local network: a UDP probe, broadcast or to one address, and the answers
//! (`nascio_protocol::discovery`). Blocking: call it from a thread of its own.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub use nascio_protocol::discovery::{DISCOVERY_PORT, ServerInfo};
use nascio_protocol::discovery::{probe, read_answer};

/// A server that answered: the address of its game (QUIC) and what it said.
#[derive(Clone, Debug)]
pub struct Found {
    pub addr: SocketAddr,
    pub info: ServerInfo,
}

/// Asks every server of the local network (broadcast, and this machine) for `wait`.
pub fn scan(wait: Duration) -> anyhow::Result<Vec<Found>> {
    let targets = [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), DISCOVERY_PORT),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DISCOVERY_PORT),
    ];
    ask(&targets, wait)
}

/// Asks the server at `ip` (its discovery port) for its game port and certificate.
pub fn query(ip: IpAddr, wait: Duration) -> anyhow::Result<Found> {
    ask(&[SocketAddr::new(ip, DISCOVERY_PORT)], wait)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no answer from {ip} (UDP port {DISCOVERY_PORT})"))
}

fn ask(targets: &[SocketAddr], wait: Duration) -> anyhow::Result<Vec<Found>> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let p = probe();
    for t in targets {
        // A broadcast may be refused (no network): the other targets still count.
        let _ = socket.send_to(&p, t);
    }
    let end = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    let mut buf = vec![0u8; 2_048];
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        socket.set_read_timeout(Some(left))?;
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            break;
        };
        let Some(info) = read_answer(&buf[..n]) else {
            continue;
        };
        let addr = SocketAddr::new(from.ip(), info.port);
        if !found.iter().any(|f| f.addr == addr) {
            found.push(Found { addr, info });
        }
    }
    found.sort_by_key(|f| f.addr);
    Ok(found)
}
