# Bedrock Edition

The aim, on this fork: Horizon joins Bedrock servers, and Bedrock clients join Horizon servers. Neither works yet.
Nobody can play across editions today. What exists is the first layer, in `crates/horizon-bedrock`.

## What works

- RakNet's offline messages: the server-list ping and the connection handshake, read and written both ways.
- `horizon-bedrock ping` and `connect` talk to a server; `horizon-bedrock listen` answers a client's ping and handshake.

It is tested by the crate's client and listener against each other on loopback, not yet against the real Bedrock
game. The first real test is to run `listen` with the protocol number of your Bedrock version and look for it in the
game's server list:

```sh
cargo run -p horizon-bedrock -- listen 19132 <protocol> <version>
```

The game protocol number changes with every Bedrock release, so it is an argument and not a constant here.

## What is left

In order, each needing the one before it:

1. **RakNet sessions**: connected datagrams, acknowledgements, resends, ordering, splitting large packets.
2. **Bedrock's packet layer**: batching, compression, and the game's own packet encoding.
3. **Login**: Xbox Live sign-in and its signed identity chain, then the encryption handshake. Horizon joining an
   online-mode server needs the player's Microsoft account to sign in for Bedrock.
4. **Translation**: Bedrock has its own numbering for blocks, items and entities, and its own chunk format. These
   tables follow the Bedrock version, and it is the bulk of the work.
5. **Bedrock clients on Horizon**: a gateway that turns a Bedrock session into a Java protocol 777 session
   (`horizon-protocol`), off by default.
6. **Horizon on Bedrock servers**: the reverse, in the client. This is the harder direction: a Bedrock server's rules
   (movement, combat, redstone) differ from vanilla Java's, which Horizon's simulation is built to match.

## Rules

Everything here comes from public descriptions of the format and from watching the real game. As in the rest of the
project ([CONTRIBUTING.md](../CONTRIBUTING.md#only-original-code)), other projects' source is not read or copied.
