//! A Bedrock client reaching Horizon: answers the server-list ping and the connection handshake.

use crate::raknet::{self, Packet};
use crate::status::ServerStatus;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Duration;

/// The largest MTU offered: below the 1500 of ordinary links, leaving room for tunnels.
pub const MAX_MTU: u16 = 1400;
/// The smallest MTU accepted, RakNet's own floor.
pub const MIN_MTU: u16 = 576;

/// What `answer` should reply to `packet`, if anything. Free of sockets, so the rules can be tested.
pub fn answer(packet: &Packet, peer: SocketAddr, status: &ServerStatus) -> Option<Packet> {
    let server_guid = status.server_guid;
    match *packet {
        Packet::UnconnectedPing { time, .. } => Some(Packet::UnconnectedPong {
            time,
            server_guid,
            status: status.to_line(),
        }),
        Packet::OpenRequest1 { protocol, .. } if protocol != raknet::PROTOCOL => {
            Some(Packet::IncompatibleProtocol {
                protocol: raknet::PROTOCOL,
                server_guid,
            })
        }
        Packet::OpenRequest1 { mtu, .. } if mtu >= MIN_MTU => Some(Packet::OpenReply1 {
            server_guid,
            mtu: mtu.min(MAX_MTU),
        }),
        Packet::OpenRequest2 { mtu, .. } if (MIN_MTU..=MAX_MTU).contains(&mtu) => {
            Some(Packet::OpenReply2 {
                server_guid,
                client: peer,
                mtu,
            })
        }
        _ => None,
    }
}

pub struct Listener {
    socket: UdpSocket,
    pub status: ServerStatus,
}

impl Listener {
    pub fn bind(address: impl ToSocketAddrs, status: ServerStatus) -> io::Result<Self> {
        Ok(Self {
            socket: UdpSocket::bind(address)?,
            status,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Waits for one datagram, up to `timeout`, and answers it. What it can't read is ignored: anyone
    /// can send anything to a UDP port. Returns what was received, or None after the timeout.
    pub fn serve_one(&self, timeout: Duration) -> io::Result<Option<(Packet, SocketAddr)>> {
        self.socket.set_read_timeout(Some(timeout))?;
        let mut buffer = [0; 2048];
        let (len, peer) = match self.socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let Ok(packet) = Packet::decode(&buffer[..len]) else {
            return Ok(None);
        };
        if let Some(reply) = answer(&packet, peer, &self.status) {
            self.socket.send_to(&reply.encode(), peer)?;
        }
        Ok(Some((packet, peer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> ServerStatus {
        ServerStatus::parse("MCPE;Horizon;100;1.0.0;0;20;77;world;Survival;1;19132;19133;").unwrap()
    }

    fn peer() -> SocketAddr {
        "10.0.0.5:5000".parse().unwrap()
    }

    #[test]
    fn a_ping_gets_the_status_and_its_own_time_back() {
        let reply = answer(
            &Packet::UnconnectedPing {
                time: 55,
                client_guid: 1,
            },
            peer(),
            &status(),
        );
        assert_eq!(
            reply,
            Some(Packet::UnconnectedPong {
                time: 55,
                server_guid: 77,
                status: status().to_line(),
            })
        );
    }

    #[test]
    fn another_raknet_version_is_told_ours() {
        let reply = answer(
            &Packet::OpenRequest1 {
                protocol: 10,
                mtu: 1400,
            },
            peer(),
            &status(),
        );
        assert_eq!(
            reply,
            Some(Packet::IncompatibleProtocol {
                protocol: raknet::PROTOCOL,
                server_guid: 77,
            })
        );
    }

    #[test]
    fn the_mtu_is_capped_and_a_tiny_one_refused() {
        let reply = |mtu| {
            answer(
                &Packet::OpenRequest1 {
                    protocol: raknet::PROTOCOL,
                    mtu,
                },
                peer(),
                &status(),
            )
        };
        assert_eq!(
            reply(1492),
            Some(Packet::OpenReply1 {
                server_guid: 77,
                mtu: MAX_MTU,
            })
        );
        assert_eq!(reply(100), None);
    }

    #[test]
    fn request_2_echoes_the_clients_address_and_a_bad_mtu_gets_nothing() {
        let request = |mtu| Packet::OpenRequest2 {
            server: "10.0.0.1:19132".parse().unwrap(),
            mtu,
            client_guid: 3,
        };
        assert_eq!(
            answer(&request(1200), peer(), &status()),
            Some(Packet::OpenReply2 {
                server_guid: 77,
                client: peer(),
                mtu: 1200,
            })
        );
        assert_eq!(answer(&request(9000), peer(), &status()), None);
    }
}
