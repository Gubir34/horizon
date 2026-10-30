//! A safe wrapper over NVIDIA PhysX (`physx-sys`) for Horizon's cosmetic physics (docs/physx.md): boxes
//! that fall, tumble and rest. Nothing here feeds back into gameplay, which stays vanilla's.
//!
//! Its own workspace, like the launcher: PhysX is C++, and only `--features physx` builds it.

use physx_sys::*;
use std::collections::HashMap;
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};

/// A box's place: where its centre is, and its rotation as a quaternion `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

/// One dynamic box in a [`Scene`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyId(u32);

/// What every scene shares. PhysX allows one foundation in a process, so this lives until it exits.
struct Engine {
    physics: *mut PxPhysics,
    material: *mut PxMaterial,
    dispatcher: *mut PxDefaultCpuDispatcher,
}

// SAFETY: every use goes through the Mutex in `engine`.
unsafe impl Send for Engine {}

fn engine() -> &'static Mutex<Engine> {
    static ENGINE: OnceLock<Mutex<Engine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        // SAFETY: runs once, and PhysX's objects are never released.
        unsafe {
            let foundation = physx_create_foundation();
            let physics = physx_create_physics(foundation);
            let material = PxPhysics_createMaterial_mut(physics, 0.6, 0.5, 0.25);
            let dispatcher = phys_PxDefaultCpuDispatcherCreate(
                1,
                null_mut(),
                PxDefaultCpuDispatcherWaitForWorkMode::WaitForWork,
                0,
            );
            Mutex::new(Engine {
                physics,
                material,
                dispatcher,
            })
        }
    })
}

fn vec3(v: [f32; 3]) -> PxVec3 {
    PxVec3 {
        x: v[0],
        y: v[1],
        z: v[2],
    }
}

fn finite(values: &[[f32; 3]]) -> bool {
    values.iter().flatten().all(|v| v.is_finite())
}

/// A world of boxes. PhysX owns the actors outside the scene, so this releases each one itself.
pub struct Scene {
    scene: *mut PxScene,
    statics: Vec<*mut PxRigidStatic>,
    bodies: HashMap<BodyId, *mut PxRigidDynamic>,
    next: u32,
}

// SAFETY: a PxScene may move between threads while nothing else uses it, and `&mut self` sees to that.
unsafe impl Send for Scene {}

impl Scene {
    /// A scene with this gravity, in metres (blocks) per second squared.
    pub fn new(gravity: [f32; 3]) -> Self {
        let engine = engine().lock().unwrap();
        // SAFETY: the engine's pointers are valid for the process, and the lock is held.
        let scene = unsafe {
            let mut desc = PxSceneDesc_new(PxPhysics_getTolerancesScale(engine.physics));
            desc.gravity = vec3(gravity);
            desc.cpuDispatcher = engine.dispatcher.cast();
            desc.filterShader = get_default_simulation_filter_shader();
            PxPhysics_createScene_mut(engine.physics, &desc)
        };
        assert!(!scene.is_null(), "PhysX could not create a scene");
        Self {
            scene,
            statics: Vec::new(),
            bodies: HashMap::new(),
            next: 0,
        }
    }

    /// A box that never moves. False if PhysX refused it (a size of zero, or NaN).
    pub fn add_static_box(&mut self, center: [f32; 3], half: [f32; 3]) -> bool {
        if !finite(&[center, half]) || half.iter().any(|h| *h <= 0.0) {
            return false;
        }
        let engine = engine().lock().unwrap();
        // SAFETY: the geometry outlives the call, which copies it; the lock is held.
        unsafe {
            let geometry = PxBoxGeometry_new(half[0], half[1], half[2]);
            let actor = phys_PxCreateStatic(
                engine.physics,
                &PxTransform_new_1(&vec3(center)),
                (&geometry as *const PxBoxGeometry).cast(),
                engine.material,
                &PxTransform_new_2(PxIDENTITY::PxIdentity),
            );
            if actor.is_null() {
                return false;
            }
            PxScene_addActor_mut(self.scene, actor.cast(), null());
            self.statics.push(actor);
        }
        true
    }

    /// Removes every static box, for a world that changed.
    pub fn clear_statics(&mut self) {
        for actor in self.statics.drain(..) {
            // SAFETY: created by this scene and released once: `drain` forgets it.
            unsafe { PxActor_release_mut(actor.cast()) };
        }
    }

    pub fn static_count(&self) -> usize {
        self.statics.len()
    }

    /// A box that falls and tumbles. None if PhysX refused it (a size of zero, or NaN).
    pub fn add_body(
        &mut self,
        center: [f32; 3],
        half: [f32; 3],
        velocity: [f32; 3],
        spin: [f32; 3],
    ) -> Option<BodyId> {
        if !finite(&[center, half, velocity, spin]) || half.iter().any(|h| *h <= 0.0) {
            return None;
        }
        let engine = engine().lock().unwrap();
        // SAFETY: the geometry outlives the call, which copies it; the lock is held.
        let actor = unsafe {
            let geometry = PxBoxGeometry_new(half[0], half[1], half[2]);
            let actor = phys_PxCreateDynamic(
                engine.physics,
                &PxTransform_new_1(&vec3(center)),
                (&geometry as *const PxBoxGeometry).cast(),
                engine.material,
                1.0,
                &PxTransform_new_2(PxIDENTITY::PxIdentity),
            );
            if actor.is_null() {
                return None;
            }
            PxRigidBody_setAngularDamping_mut(actor.cast(), 0.4);
            PxRigidDynamic_setLinearVelocity_mut(actor, &vec3(velocity), true);
            PxRigidDynamic_setAngularVelocity_mut(actor, &vec3(spin), true);
            PxScene_addActor_mut(self.scene, actor.cast(), null());
            actor
        };
        let id = BodyId(self.next);
        self.next = self.next.wrapping_add(1);
        self.bodies.insert(id, actor);
        Some(id)
    }

    /// Removes a body; an id already removed is ignored.
    pub fn remove_body(&mut self, id: BodyId) {
        if let Some(actor) = self.bodies.remove(&id) {
            // SAFETY: created by this scene and released once: it just left the map.
            unsafe { PxActor_release_mut(actor.cast()) };
        }
    }

    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    pub fn pose(&self, id: BodyId) -> Option<Pose> {
        let actor = *self.bodies.get(&id)?;
        // SAFETY: the actor is in the map, so it hasn't been released.
        let pose = unsafe { PxRigidActor_getGlobalPose(actor.cast()) };
        Some(Pose {
            position: [pose.p.x, pose.p.y, pose.p.z],
            rotation: [pose.q.x, pose.q.y, pose.q.z, pose.q.w],
        })
    }

    /// Whether a body has come to rest.
    pub fn is_sleeping(&self, id: BodyId) -> bool {
        self.bodies.get(&id).is_some_and(|actor| {
            // SAFETY: the actor is in the map, so it hasn't been released.
            unsafe { PxRigidDynamic_isSleeping(*actor) }
        })
    }

    /// Advances the world by `dt` seconds, and waits for it: a scene holds few bodies.
    pub fn step(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        // SAFETY: `&mut self` means no other simulation of this scene runs.
        unsafe {
            PxScene_simulate_mut(self.scene, dt, null_mut(), null_mut(), 0, true);
            let mut error = 0;
            PxScene_fetchResults_mut(self.scene, true, &mut error);
        }
    }
}

impl Drop for Scene {
    fn drop(&mut self) {
        self.clear_statics();
        for (_, actor) in self.bodies.drain() {
            // SAFETY: created by this scene and released once: `drain` forgets it.
            unsafe { PxActor_release_mut(actor.cast()) };
        }
        // SAFETY: nothing uses the scene after this, and its actors are gone.
        unsafe { PxScene_release_mut(self.scene) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];
    const DT: f32 = 1.0 / 20.0;
    const HALF: [f32; 3] = [0.1, 0.1, 0.1];

    /// A floor whose top is at y = 0.
    fn floored() -> Scene {
        let mut scene = Scene::new(GRAVITY);
        assert!(scene.add_static_box([0.0, -0.5, 0.0], [10.0, 0.5, 10.0]));
        scene
    }

    #[test]
    fn a_box_in_free_fall_follows_gravity() {
        let mut scene = Scene::new(GRAVITY);
        let body = scene
            .add_body([0.0, 100.0, 0.0], HALF, [0.0; 3], [0.0; 3])
            .unwrap();
        for _ in 0..20 {
            scene.step(DT);
        }
        // One second: about 4.9 down, whatever the integration's small error.
        let fallen = 100.0 - scene.pose(body).unwrap().position[1];
        assert!((4.0..6.0).contains(&fallen), "fell {fallen}");
    }

    #[test]
    fn a_box_lands_on_the_floor_and_comes_to_rest() {
        let mut scene = floored();
        let body = scene
            .add_body([0.0, 2.0, 0.0], HALF, [0.5, 0.0, 0.0], [3.0, 1.0, 0.0])
            .unwrap();
        for _ in 0..200 {
            scene.step(DT);
        }
        let y = scene.pose(body).unwrap().position[1];
        assert!((0.05..0.25).contains(&y), "rests at {y}");
        assert!(scene.is_sleeping(body));
    }

    #[test]
    fn scenes_are_separate_and_can_come_and_go() {
        for _ in 0..3 {
            let mut scene = floored();
            scene.add_body([0.0, 1.0, 0.0], HALF, [0.0; 3], [0.0; 3]);
            scene.step(DT);
        }
        let mut other = Scene::new(GRAVITY);
        let body = other
            .add_body([0.0, 1.0, 0.0], HALF, [0.0; 3], [0.0; 3])
            .unwrap();
        for _ in 0..40 {
            other.step(DT);
        }
        assert!(other.pose(body).unwrap().position[1] < 0.0, "no floor here");
    }

    #[test]
    fn a_removed_body_is_gone_and_its_id_stays_dead() {
        let mut scene = floored();
        let first = scene.add_body([0.0; 3], HALF, [0.0; 3], [0.0; 3]).unwrap();
        scene.remove_body(first);
        scene.remove_body(first);
        assert_eq!(scene.pose(first), None);
        assert_eq!(scene.body_count(), 0);
        let second = scene.add_body([0.0; 3], HALF, [0.0; 3], [0.0; 3]).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn nonsense_is_refused_not_passed_to_physx() {
        let mut scene = Scene::new(GRAVITY);
        assert_eq!(
            scene.add_body([f32::NAN, 0.0, 0.0], HALF, [0.0; 3], [0.0; 3]),
            None
        );
        assert_eq!(
            scene.add_body([0.0; 3], [0.0, 1.0, 1.0], [0.0; 3], [0.0; 3]),
            None
        );
        assert!(!scene.add_static_box([0.0; 3], [-1.0, 1.0, 1.0]));
        assert_eq!(scene.static_count(), 0);
        scene.step(f32::NAN);
        scene.step(0.0);
    }

    #[test]
    fn clearing_statics_lets_a_resting_box_fall_once_it_wakes() {
        let mut scene = floored();
        assert_eq!(scene.static_count(), 1);
        scene.clear_statics();
        assert_eq!(scene.static_count(), 0);
        let body = scene
            .add_body([0.0, 1.0, 0.0], HALF, [0.0; 3], [0.0; 3])
            .unwrap();
        for _ in 0..40 {
            scene.step(DT);
        }
        assert!(scene.pose(body).unwrap().position[1] < 0.0);
    }
}
