//! Bedrock Edition's network layer, both ways (docs/bedrock.md): Horizon reaching a Bedrock server and
//! a Bedrock client reaching Horizon. So far the part before any game packet: RakNet's offline
//! messages, the server-list ping and the connection handshake. Written from the public description
//! of the format, never from another project's source (CONTRIBUTING.md).

pub mod client;
pub mod listener;
pub mod raknet;
pub mod status;

pub use client::{Connection, connect, query_status};
pub use listener::Listener;
pub use raknet::{DecodeError, Packet};
pub use status::ServerStatus;
