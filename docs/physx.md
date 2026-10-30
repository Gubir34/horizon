# PhysX

Cosmetic physics through [NVIDIA PhysX](https://github.com/NVIDIA-Omniverse/PhysX), on this fork. So far one effect:
**physics debris**, crumbs a broken block throws that bounce and settle.

It is only for show. The world doesn't know the crumbs, they touch no entity and change no gameplay, and vanilla's own
crumbs are spawned as ever. Gameplay physics would stop Horizon matching vanilla, and clients would disagree with the
server. Effects stay on the client.

## Turning it on

It needs two things, as PhysX is a C++ library and most builds have no use for it:

```sh
cargo run --profile fast -p horizon-client --features physx -- --resources .local/horizon-client/26.3
```

Then **Options → Horizon Settings... → Extras... → Physics debris**, off by default. A build without the feature doesn't
show the setting, and doesn't need a C++ compiler or CMake.

## How it works

| Part | Code |
|---|---|
| Safe wrapper over `physx-sys` | `crates/horizon-physx`, its own workspace |
| Bursts, the scene and the world's blocks | `crates/horizon-client/src/game/debris.rs` |
| The setting | `crates/horizon-client-render/src/extras.rs` |
| Drawing | `Particle::driven`, through the ordinary particle engine |

Each tick the scene steps once, and each crumb is pushed to the particle engine for one tick, moving from its last
position to its new one. Only the blocks within three of a burst go to PhysX, rebuilt with each new burst. A burst lasts
three seconds, and eight at most live at once.

```sh
cargo test --manifest-path crates/horizon-physx/Cargo.toml
cargo test -p horizon-client --features physx --bin horizon-client debris
```

## Limits

- Crumbs are flat particle sprites: PhysX tumbles their boxes, but only the position shows. Solid cubes need a new draw.
- Collision follows each block's height at its centre, so stairs and other uneven shapes are approximate.
- PhysX runs on the CPU here. GPU acceleration isn't used: it would need NVIDIA-only binaries, and would add nothing
  at this scale.
- `physx-sys`, the Rust binding, was archived in 2024 and stays on an older PhysX. A later PhysX means maintaining the
  binding ourselves.
- This breaks the project's "everything players run is Rust" rule, which is why it stays behind a feature on this fork.
- Mob ragdolls aren't done.
