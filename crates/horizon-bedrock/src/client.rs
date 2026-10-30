//! Horizon reaching a Bedrock server: asks for its status and opens a connection.

use crate::raknet::{self, HEADERS, Packet};
use crate::status::ServerStatus;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// The sizes tried for the MTU probe, largest first: each stops at the first that gets through.
const MTU_PROBES: [u16; 3] = [1492, 1200, 576];
/// Each message is sent again this many times before giving up: UDP may lose any of them.
const ATTEMPTS: u32 = 3;

/// A connection the handshake opened. Game packets are not spoken yet (docs/bedrock.md).
#[derive(Debug)]
pub struct Connection {
    pub socket: UdpSocket,
    pub server: SocketAddr,
    pub server_guid: i64,
    pub client_guid: i64,
    pub mtu: u16,
}

fn timed_out(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, format!("no answer to {what}"))
}

fn open_socket(address: impl ToSocketAddrs) -> io::Result<(UdpSocket, SocketAddr)> {
    let server = address
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the name has no address"))?;
    let any = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(any)?;
    socket.connect(server)?;
    Ok((socket, server))
}

/// Sends `request` until `read` accepts an answer, up to [`ATTEMPTS`] times. `connect`ed, so only the
/// server's datagrams arrive; what doesn't parse or isn't the answer is skipped.
fn exchange<T>(
    socket: &UdpSocket,
    request: &[u8],
    timeout: Duration,
    mut read: impl FnMut(Packet) -> Option<io::Result<T>>,
) -> io::Result<Option<T>> {
    let mut buffer = [0; 2048];
    for _ in 0..ATTEMPTS {
        match socket.send(request) {
            Ok(_) => {}
            // Above the path's real MTU, or nobody listening yet: this attempt is lost like any other.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused | io::ErrorKind::InvalidInput
                ) || e.raw_os_error() == Some(90) => {}
            Err(e) => return Err(e),
        }
        let deadline = Instant::now() + timeout;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            socket.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
            match socket.recv(&mut buffer) {
                Ok(len) => {
                    let Ok(packet) = Packet::decode(&buffer[..len]) else {
                        continue;
                    };
                    if let Some(result) = read(packet) {
                        return result.map(Some);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    break;
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(None)
}

/// A random-enough id for this client: RakNet only needs it to differ between clients.
fn guid() -> i64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    (nanos as i64) ^ ((std::process::id() as i64) << 32)
}

/// The server's list entry and the round trip it took.
pub fn query_status(
    address: impl ToSocketAddrs,
    timeout: Duration,
) -> io::Result<(ServerStatus, Duration)> {
    let (socket, _) = open_socket(address)?;
    let started = Instant::now();
    let time = 1 + started.elapsed().as_millis() as i64;
    let ping = Packet::UnconnectedPing {
        time,
        client_guid: guid(),
    }
    .encode();
    let status = exchange(&socket, &ping, timeout, |packet| match packet {
        Packet::UnconnectedPong {
            time: echoed,
            status,
            ..
        } if echoed == time => Some(ServerStatus::parse(&status).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "not a Bedrock server's status")
        })),
        _ => None,
    })?
    .ok_or_else(|| timed_out("the ping"))?;
    Ok((status, started.elapsed()))
}

/// Runs the handshake: finds an MTU that passes, then opens the connection.
pub fn connect(address: impl ToSocketAddrs, timeout: Duration) -> io::Result<Connection> {
    let (socket, server) = open_socket(address)?;
    let mut offered = None;
    for mtu in MTU_PROBES {
        let probe = Packet::OpenRequest1 {
            protocol: raknet::PROTOCOL,
            mtu,
        }
        .encode();
        debug_assert_eq!(probe.len(), mtu as usize - HEADERS);
        offered = exchange(&socket, &probe, timeout, |packet| match packet {
            Packet::OpenReply1 { mtu, .. } => Some(Ok(mtu)),
            Packet::IncompatibleProtocol { protocol, .. } => Some(Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "the server speaks RakNet {protocol}, not {}",
                    raknet::PROTOCOL
                ),
            ))),
            _ => None,
        })?;
        if offered.is_some() {
            break;
        }
    }
    let mtu = offered.ok_or_else(|| timed_out("the connection request"))?;
    let client_guid = guid();
    let request = Packet::OpenRequest2 {
        server,
        mtu,
        client_guid,
    }
    .encode();
    let (server_guid, mtu) = exchange(&socket, &request, timeout, |packet| match packet {
        Packet::OpenReply2 {
            server_guid, mtu, ..
        } => Some(Ok((server_guid, mtu))),
        _ => None,
    })?
    .ok_or_else(|| timed_out("the second connection request"))?;
    Ok(Connection {
        socket,
        server,
        server_guid,
        client_guid,
        mtu,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listener::{Listener, MAX_MTU};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Runs a real listener on a loopback port for as long as the test holds the guard.
    struct Running(Arc<AtomicBool>, Option<std::thread::JoinHandle<()>>);

    impl Drop for Running {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
            let _ = self.1.take().map(std::thread::JoinHandle::join);
        }
    }

    fn listener() -> (SocketAddr, Running) {
        let status =
            ServerStatus::parse("MCPE;Horizon;100;1.0.0;3;20;77;world;Survival;1;0;0;").unwrap();
        let listener = Listener::bind("127.0.0.1:0", status).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                listener.serve_one(Duration::from_millis(20)).unwrap();
            }
        });
        (address, Running(stop, Some(thread)))
    }

    #[test]
    fn the_client_reads_a_listeners_status_over_udp() {
        let (address, _running) = listener();
        let (status, _) = query_status(address, Duration::from_secs(2)).unwrap();
        assert_eq!(status.name, "Horizon");
        assert_eq!((status.players, status.max_players), (3, 20));
    }

    #[test]
    fn the_client_opens_a_connection_to_a_listener_over_udp() {
        let (address, _running) = listener();
        let connection = connect(address, Duration::from_secs(2)).unwrap();
        assert_eq!(connection.server_guid, 77);
        assert_eq!(connection.server, address);
        assert_eq!(connection.mtu, MAX_MTU);
    }

    #[test]
    fn silence_is_a_timeout_not_a_hang() {
        // Bound and never read: the datagrams go nowhere.
        let quiet = UdpSocket::bind("127.0.0.1:0").unwrap();
        let error =
            query_status(quiet.local_addr().unwrap(), Duration::from_millis(20)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
