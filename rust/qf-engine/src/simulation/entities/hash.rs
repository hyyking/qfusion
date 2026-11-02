//! Canonical state hash of the entity world, for determinism checks.
//!
//! Entities are visited in lump order and ECS entity ids are never hashed, so the
//! result does not depend on spawn layout, archetype order or query iteration order.

use std::fmt::Debug;

use bevy_ecs::prelude::*;

use super::components::{
    Classname, Enabled, Level, Links, LumpIndex, Occupants, Outbox, Placement, Registry, Think,
    Timing, TouchKind, UseKind, Volume,
};
use super::movers::Movers;

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(FNV_PRIME);
        }
    }

    fn text(&mut self, value: &impl Debug) {
        self.bytes(format!("{value:?}").as_bytes());
        // Field separator so adjacent values cannot run together.
        self.bytes(&[0xFF]);
    }
}

fn component<T: Component + Debug>(hash: &mut Fnv, world: &World, entity: Entity) {
    match world.get::<T>(entity) {
        Some(value) => hash.text(value),
        None => hash.bytes(&[0]),
    }
}

pub(super) fn state_hash(world: &World) -> u64 {
    let mut hash = Fnv(FNV_OFFSET);
    let level = world.resource::<Level>();
    hash.text(&(level.time_ms, level.tick, level.gravity.to_bits()));

    for slot in &world.resource::<Registry>().by_lump {
        let Some(entity) = *slot else {
            hash.bytes(&[1]);
            continue;
        };
        component::<LumpIndex>(&mut hash, world, entity);
        component::<Classname>(&mut hash, world, entity);
        component::<Links>(&mut hash, world, entity);
        component::<Placement>(&mut hash, world, entity);
        component::<Volume>(&mut hash, world, entity);
        component::<Timing>(&mut hash, world, entity);
        component::<Enabled>(&mut hash, world, entity);
        component::<TouchKind>(&mut hash, world, entity);
        component::<UseKind>(&mut hash, world, entity);
        component::<Think>(&mut hash, world, entity);
    }

    if let Some(movers) = world.get_resource::<Movers>() {
        for mover in &movers.list {
            hash.text(&mover.canonical());
        }
    }
    hash.text(&world.resource::<Occupants>().0);
    hash.text(&world.resource::<Outbox>().events);
    hash.0
}
