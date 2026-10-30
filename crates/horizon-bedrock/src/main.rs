//! Tries the Bedrock layer against real programs, before the game speaks it (docs/bedrock.md):
//!
//! ```sh
//! horizon-bedrock ping <host[:port]>                  # a Bedrock server's list entry and round trip
//! horizon-bedrock connect <host[:port]>               # runs the handshake and reports the MTU
//! horizon-bedrock listen <port> <protocol> <version>  # shows up in a Bedrock client's server list
//! ```

use horizon_bedrock::{Listener, ServerStatus, connect, query_status};
use std::time::Duration;

/// The port Bedrock servers use unless they say otherwise.
const DEFAULT_PORT: u16 = 19132;
const TIMEOUT: Duration = Duration::from_secs(2);

fn with_port(address: &str) -> String {
    if address.contains(':') {
        address.to_owned()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["ping", address] => {
            let (status, took) =
                query_status(with_port(address), TIMEOUT).map_err(|e| e.to_string())?;
            println!(
                "{} ({} players of {}), Bedrock {} (protocol {}), {} ms",
                status.name,
                status.players,
                status.max_players,
                status.version,
                status.protocol,
                took.as_millis()
            );
            Ok(())
        }
        ["connect", address] => {
            let connection = connect(with_port(address), TIMEOUT).map_err(|e| e.to_string())?;
            println!(
                "handshake done with {}: guid {}, MTU {}. Game packets are not spoken yet.",
                connection.server, connection.server_guid, connection.mtu
            );
            Ok(())
        }
        ["listen", port, protocol, version] => {
            let port = port.parse().map_err(|_| "the port must be a number")?;
            let status = ServerStatus {
                name: "Horizon".into(),
                protocol: protocol.parse().map_err(|_| "the protocol must be a number")?,
                version: (*version).to_owned(),
                players: 0,
                max_players: 20,
                server_guid: std::process::id() as i64,
                world: "Horizon".into(),
                game_mode: "Survival".into(),
                port_v4: port,
                port_v6: port,
            };
            let listener =
                Listener::bind(("0.0.0.0", port), status).map_err(|e| e.to_string())?;
            println!("listening on {port}/udp, joining isn't possible yet");
            loop {
                if let Some((packet, peer)) = listener
                    .serve_one(Duration::from_secs(1))
                    .map_err(|e| e.to_string())?
                {
                    println!("{peer}: {packet:?}");
                }
            }
        }
        _ => Err("usage: horizon-bedrock ping <host[:port]> | connect <host[:port]> | listen <port> <protocol> <version>".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
