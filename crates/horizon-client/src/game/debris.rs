//! Horizon Extras' physics debris (docs/physx.md): a broken block also throws crumbs that PhysX moves.
//! Only for show. The world doesn't know them, and vanilla's own crumbs are spawned as ever.
//!
//! The engine draws each crumb one tick at a time ([`Particle::driven`]), so its rules stay vanilla's.
use super::{BlockStateId, Game};
use horizon_client::particles::Particle;
use horizon_core::{geometry::Axis, random::RandomSource};
use horizon_physx::{BodyId, Scene};
use std::collections::{HashSet, VecDeque};

/// The crumbs one broken block throws, one from each corner of the block.
const PIECES: usize = 8;
/// Bursts alive at once: a new one past this removes the oldest, so strip mining stays cheap.
const MAX_BURSTS: usize = 8;
/// How long a burst lasts, in ticks.
const LIFETIME: u32 = 60;
const HALF: f32 = 0.05;
/// How far from a burst the world's blocks are given to PhysX.
const REACH: i32 = 3;
/// Vanilla's 0.04 blocks per tick per tick, in seconds.
const GRAVITY: [f32; 3] = [0.0, -16.0, 0.0];
const TICK: f32 = 1.0 / 20.0;
/// Past this from the scene's anchor, an f32 position is too coarse to trust.
const MAX_FROM_ANCHOR: i32 = 512;

struct Piece {
    body: BodyId,
    crumb: Particle,
    last: [f64; 3],
}

struct Burst {
    origin: [i32; 3],
    age: u32,
    pieces: Vec<Piece>,
}

/// A crumb to throw, with its velocity and spin per second.
struct Throw {
    crumb: Particle,
    at: [f64; 3],
    velocity: [f32; 3],
    spin: [f32; 3],
}

/// The scene and what lives in it. PhysX works in f32, too coarse for the world's far coordinates, so
/// the scene's positions count from `anchor`, the first burst's block.
#[derive(Default)]
pub(super) struct Debris {
    scene: Option<Scene>,
    anchor: [i32; 3],
    bursts: VecDeque<Burst>,
}

impl Debris {
    fn local(&self, at: [f64; 3]) -> [f32; 3] {
        std::array::from_fn(|i| (at[i] - f64::from(self.anchor[i])) as f32)
    }

    fn release(&mut self, burst: Burst) {
        if let Some(scene) = &mut self.scene {
            for piece in burst.pieces {
                scene.remove_body(piece.body);
            }
        }
    }

    fn clear(&mut self) {
        for burst in std::mem::take(&mut self.bursts) {
            self.release(burst);
        }
        if let Some(scene) = &mut self.scene {
            scene.clear_statics();
        }
    }

    /// Adds a burst for the block broken at `origin`. `top` gives a block's collision height, 0 for
    /// none; the blocks around every live burst go to PhysX again, as the world may have changed.
    fn add_burst(&mut self, origin: [i32; 3], throws: Vec<Throw>, top: impl Fn([i32; 3]) -> f32) {
        let far = (0..3).any(|i| (origin[i] - self.anchor[i]).abs() > MAX_FROM_ANCHOR);
        if far {
            self.clear();
        }
        if self.bursts.is_empty() {
            self.anchor = origin;
        }
        while self.bursts.len() >= MAX_BURSTS {
            if let Some(oldest) = self.bursts.pop_front() {
                self.release(oldest);
            }
        }
        let locals: Vec<[f32; 3]> = throws.iter().map(|t| self.local(t.at)).collect();
        let scene = self.scene.get_or_insert_with(|| Scene::new(GRAVITY));
        let half = [HALF; 3];
        let pieces = throws
            .into_iter()
            .zip(locals)
            .filter_map(|(throw, at)| {
                let body = scene.add_body(at, half, throw.velocity, throw.spin)?;
                Some(Piece {
                    body,
                    crumb: throw.crumb,
                    last: throw.at,
                })
            })
            .collect();
        self.bursts.push_back(Burst {
            origin,
            age: 0,
            pieces,
        });
        self.rebuild_statics(top);
    }

    fn rebuild_statics(&mut self, top: impl Fn([i32; 3]) -> f32) {
        let origins: HashSet<[i32; 3]> = self.bursts.iter().map(|b| b.origin).collect();
        let anchor = self.anchor;
        let Some(scene) = &mut self.scene else {
            return;
        };
        scene.clear_statics();
        let mut seen = HashSet::new();
        for origin in &origins {
            for dx in -REACH..=REACH {
                for dy in -REACH..=REACH {
                    for dz in -REACH..=REACH {
                        let at = [origin[0] + dx, origin[1] + dy, origin[2] + dz];
                        // A broken block may not have left the world yet when its event arrives.
                        if origins.contains(&at) || !seen.insert(at) {
                            continue;
                        }
                        let height = top(at);
                        if height > 0.0 {
                            let from = |i: usize| (at[i] - anchor[i]) as f32;
                            scene.add_static_box(
                                [from(0) + 0.5, from(1) + height / 2.0, from(2) + 0.5],
                                [0.5, height / 2.0, 0.5],
                            );
                        }
                    }
                }
            }
        }
    }

    /// One tick: moves everything, and hands the engine each crumb to draw until the next.
    fn tick(&mut self, mut draw: impl FnMut(Particle)) {
        if self.bursts.is_empty() {
            return;
        }
        let Some(scene) = &mut self.scene else {
            return;
        };
        scene.step(TICK);
        let anchor = self.anchor.map(f64::from);
        for burst in &mut self.bursts {
            burst.age += 1;
            for piece in &mut burst.pieces {
                let Some(pose) = scene.pose(piece.body) else {
                    continue;
                };
                let at = std::array::from_fn(|i| f64::from(pose.position[i]) + anchor[i]);
                draw(piece.crumb.driven(piece.last, at));
                piece.last = at;
            }
        }
        while self.bursts.front().is_some_and(|b| b.age >= LIFETIME) {
            if let Some(done) = self.bursts.pop_front() {
                self.release(done);
            }
        }
        if self.bursts.is_empty()
            && let Some(scene) = &mut self.scene
        {
            scene.clear_statics();
        }
    }
}

impl Game<'_> {
    /// The crumbs of a block just broken at `pos`. Nothing without the extra, or without a window: the
    /// server thread's `Game` sees the same events.
    pub(super) fn spawn_debris(&mut self, pos: [i32; 3], sprite: &str, tint: u32) {
        if !self.settings.extras.physics_debris || self.renderer.is_none() {
            return;
        }
        let r = self.particles.random();
        let throws = (0..PIECES)
            .map(|corner| {
                let side = |bit: usize| if corner >> bit & 1 == 1 { 1.0f32 } else { -1.0 };
                let at = [
                    f64::from(pos[0]) + 0.5 + f64::from(side(0)) * 0.25,
                    f64::from(pos[1]) + 0.5 + f64::from(side(1)) * 0.25,
                    f64::from(pos[2]) + 0.5 + f64::from(side(2)) * 0.25,
                ];
                let velocity = [
                    side(0) * (0.5 + r.next_float() * 2.0),
                    1.5 + r.next_float() * 3.0,
                    side(2) * (0.5 + r.next_float() * 2.0),
                ];
                let spin = std::array::from_fn(|_| (r.next_float() - 0.5) * 16.0);
                let mut crumb = Particle::terrain(r, at, [0.0; 3], sprite.to_owned(), tint);
                crumb.scale(1.5);
                Throw {
                    crumb,
                    at,
                    velocity,
                    spin,
                }
            })
            .collect();
        let (world, facts, collision) = (&self.world, self.res.facts(), self.collision);
        self.debris.add_burst(pos, throws, |at| {
            let state: BlockStateId = world.block(at[0], at[1], at[2]);
            // As `ParticleLevel::collision_top` reads it; a fence's 1.5 is fine for a crumb.
            match collision.cached_shapes().cached_shape(state) {
                Ok(Some(shape)) if !shape.is_empty() => shape.max_at(Axis::Y, 0.5, 0.5) as f32,
                Ok(Some(_)) => 0.0,
                _ if facts.get(state).full_collision => 1.0,
                _ => 0.0,
            }
        });
    }

    pub(super) fn tick_debris(&mut self) {
        let particles = &mut self.particles;
        self.debris.tick(|crumb| particles.push(crumb));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::random::LegacyRandom;

    /// Solid up to and including y = 63.
    fn floor(at: [i32; 3]) -> f32 {
        if at[1] <= 63 { 1.0 } else { 0.0 }
    }

    fn throws(origin: [i32; 3]) -> Vec<Throw> {
        let mut r = LegacyRandom::new(1);
        (0..PIECES)
            .map(|i| {
                let at = [
                    f64::from(origin[0]) + 0.25 + 0.5 * (i & 1) as f64,
                    f64::from(origin[1]) + 0.5,
                    f64::from(origin[2]) + 0.25 + 0.5 * (i >> 1 & 1) as f64,
                ];
                Throw {
                    crumb: Particle::terrain(&mut r, at, [0.0; 3], "stone".into(), 0xffffff),
                    at,
                    velocity: [0.5, 2.0, 0.0],
                    spin: [4.0, 0.0, 1.0],
                }
            })
            .collect()
    }

    fn positions(debris: &Debris) -> Vec<[f64; 3]> {
        let scene = debris.scene.as_ref().unwrap();
        debris
            .bursts
            .iter()
            .flat_map(|b| &b.pieces)
            .map(|p| {
                let at = scene.pose(p.body).unwrap().position;
                std::array::from_fn(|i| f64::from(at[i]) + f64::from(debris.anchor[i]))
            })
            .collect()
    }

    #[test]
    fn crumbs_settle_on_the_ground_where_they_were_thrown() {
        let mut debris = Debris::default();
        let origin = [10, 64, -20];
        debris.add_burst(origin, throws(origin), floor);
        for _ in 0..LIFETIME - 1 {
            debris.tick(|_| {});
        }
        for at in positions(&debris) {
            assert!((64.0..64.2).contains(&at[1]), "at {at:?}");
            assert!((at[0] - 10.5).abs() < 3.0, "at {at:?}");
        }
    }

    #[test]
    fn far_from_the_world_origin_they_still_settle() {
        let mut debris = Debris::default();
        let origin = [20_000_000, 64, -20_000_000];
        debris.add_burst(origin, throws(origin), floor);
        for _ in 0..LIFETIME - 1 {
            debris.tick(|_| {});
        }
        for at in positions(&debris) {
            assert!((64.0..64.2).contains(&at[1]), "at {at:?}");
            assert!((at[0] - 20_000_000.5).abs() < 3.0, "at {at:?}");
        }
    }

    #[test]
    fn each_crumb_is_drawn_once_a_tick_moving_from_where_it_was() {
        let mut debris = Debris::default();
        let origin = [0, 64, 0];
        debris.add_burst(origin, throws(origin), floor);
        let mut first = Vec::new();
        debris.tick(|c| first.push(c));
        assert_eq!(first.len(), PIECES);
        assert_eq!(first[0].previous, [0.25, 64.5, 0.25]);
        assert_ne!(first[0].position, first[0].previous);
        assert_eq!(first[0].lifetime, 0);
        let mut second = Vec::new();
        debris.tick(|c| second.push(c));
        assert_eq!(second[0].previous, first[0].position);
    }

    #[test]
    fn a_burst_ends_and_leaves_nothing_in_the_scene() {
        let mut debris = Debris::default();
        let origin = [0, 64, 0];
        debris.add_burst(origin, throws(origin), floor);
        for _ in 0..LIFETIME {
            debris.tick(|_| {});
        }
        assert!(debris.bursts.is_empty());
        let scene = debris.scene.as_ref().unwrap();
        assert_eq!((scene.body_count(), scene.static_count()), (0, 0));
        let mut drawn = 0;
        debris.tick(|_| drawn += 1);
        assert_eq!(drawn, 0);
    }

    #[test]
    fn the_oldest_burst_makes_way() {
        let mut debris = Debris::default();
        for x in 0..MAX_BURSTS as i32 + 3 {
            let origin = [x * 2, 64, 0];
            debris.add_burst(origin, throws(origin), floor);
        }
        assert_eq!(debris.bursts.len(), MAX_BURSTS);
        assert_eq!(debris.bursts[0].origin, [6, 64, 0]);
        let bodies = debris.scene.as_ref().unwrap().body_count();
        assert_eq!(bodies, MAX_BURSTS * PIECES);
    }

    #[test]
    fn the_broken_block_itself_is_never_solid() {
        let mut debris = Debris::default();
        let origin = [0, 64, 0];
        debris.add_burst(
            origin,
            throws(origin),
            |at| {
                if at == origin { 1.0 } else { 0.0 }
            },
        );
        assert_eq!(debris.scene.as_ref().unwrap().static_count(), 0);
    }

    #[test]
    fn a_burst_far_away_starts_over_rather_than_losing_precision() {
        let mut debris = Debris::default();
        let near = [0, 64, 0];
        debris.add_burst(near, throws(near), floor);
        let far = [100_000, 64, 0];
        debris.add_burst(far, throws(far), floor);
        assert_eq!(debris.bursts.len(), 1);
        assert_eq!(debris.anchor, far);
        assert_eq!(debris.scene.as_ref().unwrap().body_count(), PIECES);
    }
}
