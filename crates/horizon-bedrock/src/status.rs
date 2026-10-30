//! What a Bedrock server says in its pong, the line a client's server list shows:
//! `MCPE;<name>;<protocol>;<version>;<players>;<max>;<guid>;<world>;<mode>;<mode id>;<port>;<port v6>;`

/// A server's entry in the list. `protocol` is the Bedrock game protocol, which changes with every
/// Bedrock release; a client shows a server on another protocol as outdated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub protocol: u32,
    pub version: String,
    pub players: u32,
    pub max_players: u32,
    pub server_guid: i64,
    pub world: String,
    pub game_mode: String,
    pub port_v4: u16,
    pub port_v6: u16,
}

impl ServerStatus {
    /// The pong's text. `;` separates the fields, so a name can't carry one.
    pub fn to_line(&self) -> String {
        let clean = |text: &str| text.replace(';', ":");
        let mode_id = match self.game_mode.as_str() {
            "Creative" => 0,
            "Adventure" => 2,
            _ => 1,
        };
        format!(
            "MCPE;{};{};{};{};{};{};{};{};{};{};{};",
            clean(&self.name),
            self.protocol,
            clean(&self.version),
            self.players,
            self.max_players,
            self.server_guid,
            clean(&self.world),
            clean(&self.game_mode),
            mode_id,
            self.port_v4,
            self.port_v6,
        )
    }

    /// Reads a pong's text. Only the first six fields are required: servers leave the rest out.
    pub fn parse(line: &str) -> Option<Self> {
        let mut fields = line.split(';');
        if !matches!(fields.next()?, "MCPE" | "MCEE") {
            return None;
        }
        let name = fields.next()?.to_owned();
        let protocol = fields.next()?.parse().ok()?;
        let version = fields.next()?.to_owned();
        let players = fields.next()?.parse().ok()?;
        let max_players = fields.next()?.parse().ok()?;
        let server_guid = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
        let world = fields.next().unwrap_or_default().to_owned();
        let game_mode = fields.next().unwrap_or_default().to_owned();
        fields.next(); // the mode's id: the name above says it
        let port_v4 = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
        let port_v6 = fields.next().and_then(|f| f.parse().ok()).unwrap_or(0);
        Some(Self {
            name,
            protocol,
            version,
            players,
            max_players,
            server_guid,
            world,
            game_mode,
            port_v4,
            port_v6,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> ServerStatus {
        ServerStatus {
            name: "Horizon".into(),
            protocol: 100,
            version: "1.0.0".into(),
            players: 2,
            max_players: 20,
            server_guid: -42,
            world: "world".into(),
            game_mode: "Survival".into(),
            port_v4: 19132,
            port_v6: 19133,
        }
    }

    #[test]
    fn the_line_has_the_fields_in_order() {
        assert_eq!(
            status().to_line(),
            "MCPE;Horizon;100;1.0.0;2;20;-42;world;Survival;1;19132;19133;"
        );
        assert_eq!(ServerStatus::parse(&status().to_line()), Some(status()));
    }

    #[test]
    fn a_separator_in_a_name_cannot_shift_the_fields() {
        let mut named = status();
        named.name = "a;b".into();
        assert_eq!(ServerStatus::parse(&named.to_line()).unwrap().name, "a:b");
    }

    #[test]
    fn a_short_line_is_enough_and_garbage_is_refused() {
        let short = ServerStatus::parse("MCPE;S;100;1.0.0;0;10").unwrap();
        assert_eq!((short.name.as_str(), short.max_players), ("S", 10));
        assert_eq!(short.port_v4, 0);
        assert_eq!(ServerStatus::parse("HTTP/1.1 200 OK"), None);
        assert_eq!(ServerStatus::parse("MCPE;S;not a number;1;0;10"), None);
    }
}
