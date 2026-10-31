//! RakNet's offline messages: what is sent over UDP before a connection exists. Numbers are
//! big-endian. Every message carries [`MAGIC`], which tells them from a connected session's datagrams.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// The bytes that mark an offline message.
pub const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

/// The RakNet protocol version Bedrock speaks.
pub const PROTOCOL: u8 = 11;

/// What a UDP datagram costs beyond its payload: the IPv4 header and the UDP header. An MTU counts
/// them, and [`Packet::OpenRequest1`] says its MTU by its own size.
pub const HEADERS: usize = 28;

const UNCONNECTED_PING: u8 = 0x01;
const UNCONNECTED_PONG: u8 = 0x1c;
const OPEN_REQUEST_1: u8 = 0x05;
const OPEN_REPLY_1: u8 = 0x06;
const OPEN_REQUEST_2: u8 = 0x07;
const OPEN_REPLY_2: u8 = 0x08;
const INCOMPATIBLE_PROTOCOL: u8 = 0x19;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    /// Who is there? `time` comes back in the pong, so the asker can measure the round trip.
    UnconnectedPing {
        time: i64,
        client_guid: i64,
    },
    /// `status` is [`crate::ServerStatus`]'s text.
    UnconnectedPong {
        time: i64,
        server_guid: i64,
        status: String,
    },
    /// Padded with zeros, so that arriving at all proves a datagram of `mtu` passes.
    OpenRequest1 {
        protocol: u8,
        mtu: u16,
    },
    OpenReply1 {
        server_guid: i64,
        mtu: u16,
    },
    OpenRequest2 {
        server: SocketAddr,
        mtu: u16,
        client_guid: i64,
    },
    OpenReply2 {
        server_guid: i64,
        client: SocketAddr,
        mtu: u16,
    },
    IncompatibleProtocol {
        protocol: u8,
        server_guid: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The datagram ends before the message does.
    Truncated,
    /// Not an offline message: an id this module doesn't know, or the wrong magic.
    NotOffline,
    /// An address family that is neither 4 nor 6.
    BadAddress,
    /// The pong's status isn't UTF-8.
    BadText,
    /// A request 1 too small to be a real MTU probe.
    BadSize,
    /// A connected-layer message whose fields don't make sense (named).
    Malformed(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Truncated => "the datagram ends early",
            Self::NotOffline => "not a RakNet offline message",
            Self::BadAddress => "unknown address family",
            Self::BadText => "the status text isn't UTF-8",
            Self::BadSize => "an MTU probe of an impossible size",
            Self::Malformed(what) => what,
        })
    }
}

impl std::error::Error for DecodeError {}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::UnconnectedPing { time, client_guid } => {
                out.push(UNCONNECTED_PING);
                out.extend_from_slice(&time.to_be_bytes());
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&client_guid.to_be_bytes());
            }
            Self::UnconnectedPong {
                time,
                server_guid,
                status,
            } => {
                out.push(UNCONNECTED_PONG);
                out.extend_from_slice(&time.to_be_bytes());
                out.extend_from_slice(&server_guid.to_be_bytes());
                out.extend_from_slice(&MAGIC);
                // A datagram is far shorter than u16::MAX, and so must the status be to fit one.
                let bytes = &status.as_bytes()[..status.len().min(u16::MAX as usize)];
                out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                out.extend_from_slice(bytes);
            }
            Self::OpenRequest1 { protocol, mtu } => {
                out.push(OPEN_REQUEST_1);
                out.extend_from_slice(&MAGIC);
                out.push(*protocol);
                out.resize((*mtu as usize).saturating_sub(HEADERS).max(out.len()), 0);
            }
            Self::OpenReply1 { server_guid, mtu } => {
                out.push(OPEN_REPLY_1);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                out.push(0); // no RakNet security
                out.extend_from_slice(&mtu.to_be_bytes());
            }
            Self::OpenRequest2 {
                server,
                mtu,
                client_guid,
            } => {
                out.push(OPEN_REQUEST_2);
                out.extend_from_slice(&MAGIC);
                write_address(&mut out, server);
                out.extend_from_slice(&mtu.to_be_bytes());
                out.extend_from_slice(&client_guid.to_be_bytes());
            }
            Self::OpenReply2 {
                server_guid,
                client,
                mtu,
            } => {
                out.push(OPEN_REPLY_2);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
                write_address(&mut out, client);
                out.extend_from_slice(&mtu.to_be_bytes());
                out.push(0); // no RakNet encryption
            }
            Self::IncompatibleProtocol {
                protocol,
                server_guid,
            } => {
                out.push(INCOMPATIBLE_PROTOCOL);
                out.push(*protocol);
                out.extend_from_slice(&MAGIC);
                out.extend_from_slice(&server_guid.to_be_bytes());
            }
        }
        out
    }

    pub fn decode(datagram: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader(datagram);
        let id = r.u8()?;
        match id {
            UNCONNECTED_PING => {
                let time = r.i64()?;
                r.magic()?;
                Ok(Self::UnconnectedPing {
                    time,
                    client_guid: r.i64()?,
                })
            }
            UNCONNECTED_PONG => {
                let time = r.i64()?;
                let server_guid = r.i64()?;
                r.magic()?;
                let len = r.u16()? as usize;
                let status = std::str::from_utf8(r.take(len)?).map_err(|_| DecodeError::BadText)?;
                Ok(Self::UnconnectedPong {
                    time,
                    server_guid,
                    status: status.to_owned(),
                })
            }
            OPEN_REQUEST_1 => {
                r.magic()?;
                let protocol = r.u8()?;
                let mtu =
                    u16::try_from(datagram.len() + HEADERS).map_err(|_| DecodeError::BadSize)?;
                Ok(Self::OpenRequest1 { protocol, mtu })
            }
            OPEN_REPLY_1 => {
                r.magic()?;
                let server_guid = r.i64()?;
                r.u8()?; // security: Bedrock uses none
                Ok(Self::OpenReply1 {
                    server_guid,
                    mtu: r.u16()?,
                })
            }
            OPEN_REQUEST_2 => {
                r.magic()?;
                let server = r.address()?;
                let mtu = r.u16()?;
                Ok(Self::OpenRequest2 {
                    server,
                    mtu,
                    client_guid: r.i64()?,
                })
            }
            OPEN_REPLY_2 => {
                r.magic()?;
                let server_guid = r.i64()?;
                let client = r.address()?;
                let mtu = r.u16()?;
                r.u8()?; // encryption: Bedrock's own comes later, in its login
                Ok(Self::OpenReply2 {
                    server_guid,
                    client,
                    mtu,
                })
            }
            INCOMPATIBLE_PROTOCOL => {
                let protocol = r.u8()?;
                r.magic()?;
                Ok(Self::IncompatibleProtocol {
                    protocol,
                    server_guid: r.i64()?,
                })
            }
            _ => Err(DecodeError::NotOffline),
        }
    }
}

/// An IPv4 address has its bytes inverted; an IPv6 one is laid out as a `sockaddr_in6`, whose family
/// (`AF_INET6`, 23 as RakNet writes it) is little-endian.
fn write_address(out: &mut Vec<u8>, address: &SocketAddr) {
    match address.ip() {
        IpAddr::V4(ip) => {
            out.push(4);
            out.extend(ip.octets().map(|b| !b));
            out.extend_from_slice(&address.port().to_be_bytes());
        }
        IpAddr::V6(ip) => {
            out.push(6);
            out.extend_from_slice(&23u16.to_le_bytes());
            out.extend_from_slice(&address.port().to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes()); // flow info
            out.extend_from_slice(&ip.octets());
            out.extend_from_slice(&0u32.to_be_bytes()); // scope
        }
    }
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    fn magic(&mut self) -> Result<(), DecodeError> {
        if self.take(MAGIC.len())? == MAGIC {
            Ok(())
        } else {
            Err(DecodeError::NotOffline)
        }
    }

    fn address(&mut self) -> Result<SocketAddr, DecodeError> {
        match self.u8()? {
            4 => {
                let ip: [u8; 4] = self.array()?;
                let port = self.u16()?;
                Ok(SocketAddr::new(Ipv4Addr::from(ip.map(|b| !b)).into(), port))
            }
            6 => {
                self.take(2)?; // family
                let port = self.u16()?;
                self.take(4)?; // flow info
                let ip: [u8; 16] = self.array()?;
                self.take(4)?; // scope
                Ok(SocketAddr::new(Ipv6Addr::from(ip).into(), port))
            }
            _ => Err(DecodeError::BadAddress),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(packet: Packet) {
        assert_eq!(Packet::decode(&packet.encode()), Ok(packet));
    }

    #[test]
    fn every_message_survives_a_round_trip() {
        let v4: SocketAddr = "192.168.1.20:19132".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::7]:19133".parse().unwrap();
        roundtrip(Packet::UnconnectedPing {
            time: 1234,
            client_guid: -5,
        });
        roundtrip(Packet::UnconnectedPong {
            time: 1234,
            server_guid: i64::MIN,
            status: "MCPE;Sunucu ğüş;1;2;3;4;5;;Survival;1;19132;19133;".into(),
        });
        roundtrip(Packet::OpenRequest1 {
            protocol: PROTOCOL,
            mtu: 1400,
        });
        roundtrip(Packet::OpenReply1 {
            server_guid: 9,
            mtu: 1400,
        });
        for address in [v4, v6] {
            roundtrip(Packet::OpenRequest2 {
                server: address,
                mtu: 1200,
                client_guid: 8,
            });
            roundtrip(Packet::OpenReply2 {
                server_guid: 7,
                client: address,
                mtu: 1200,
            });
        }
        roundtrip(Packet::IncompatibleProtocol {
            protocol: PROTOCOL,
            server_guid: 6,
        });
    }

    #[test]
    fn a_ping_is_laid_out_byte_by_byte() {
        let bytes = Packet::UnconnectedPing {
            time: 1,
            client_guid: 2,
        }
        .encode();
        assert_eq!(bytes.len(), 33);
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes[1..9], 1i64.to_be_bytes());
        assert_eq!(bytes[9..25], MAGIC);
        assert_eq!(bytes[25..33], 2i64.to_be_bytes());
    }

    #[test]
    fn a_request_1_says_its_mtu_by_its_size() {
        let bytes = Packet::OpenRequest1 {
            protocol: PROTOCOL,
            mtu: 1200,
        }
        .encode();
        assert_eq!(bytes.len(), 1200 - HEADERS);
    }

    #[test]
    fn an_ipv4_address_is_inverted_on_the_wire() {
        let bytes = Packet::OpenReply2 {
            server_guid: 0,
            client: "1.2.3.4:256".parse().unwrap(),
            mtu: 0,
        }
        .encode();
        assert_eq!(bytes[25..32], [4, !1, !2, !3, !4, 1, 0]);
    }

    #[test]
    fn broken_datagrams_are_refused_not_panicked_on() {
        assert_eq!(Packet::decode(&[]), Err(DecodeError::Truncated));
        assert_eq!(Packet::decode(&[0x84, 0, 0]), Err(DecodeError::NotOffline));
        let mut wrong_magic = Packet::UnconnectedPing {
            time: 1,
            client_guid: 2,
        }
        .encode();
        wrong_magic[9] ^= 1;
        assert_eq!(Packet::decode(&wrong_magic), Err(DecodeError::NotOffline));
        let whole = Packet::OpenReply2 {
            server_guid: 1,
            client: "[::1]:1".parse().unwrap(),
            mtu: 1,
        }
        .encode();
        for len in 0..whole.len() {
            assert!(Packet::decode(&whole[..len]).is_err(), "length {len}");
        }
    }
}
