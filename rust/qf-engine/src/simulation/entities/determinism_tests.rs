//! Determinism helpers of the entity world: one clock, seeded substreams, state hash.

use bevy_ecs::prelude::Component;

use super::*;
use crate::simulation::pmove::{PmTrace, PmoveWorld};

struct Models;

impl BrushModels for Models {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        match index {
            0 => Some(([0.0; 3], [0.0; 3])),
            1 => Some(([0.0; 3], [64.0, 64.0, 128.0])),
            _ => None,
        }
    }

    fn box_touches_model(&self, _: usize, _: Vec3, _: Vec3, _: Vec3) -> bool {
        false
    }
}

struct Void;

impl PmoveWorld for Void {
    fn trace(&self, _: Vec3, _: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
        PmTrace::miss(end)
    }

    fn point_contents(&self, _: Vec3) -> u32 {
        0
    }
}

fn ent(properties: &[(&str, &str)]) -> Entity {
    Entity {
        properties: properties
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

fn lump() -> Vec<Entity> {
    vec![
        ent(&[("classname", "worldspawn")]),
        ent(&[
            ("classname", "func_door"),
            ("model", "*1"),
            ("speed", "100"),
        ]),
        ent(&[
            ("classname", "target_delay"),
            ("targetname", "d"),
            ("wait", "1"),
        ]),
        ent(&[("classname", "info_null")]),
        ent(&[("classname", "trigger_always"), ("target", "d")]),
        ent(&[("classname", "target_position"), ("targetname", "p")]),
    ]
}

/// Same lump, but the ECS world already holds unrelated entities (and freed ids) so
/// entity ids and archetype layout differ.
fn churned(lump: &[Entity]) -> MapEntities {
    #[derive(Component)]
    struct Junk(#[allow(dead_code)] u32);
    #[derive(Component)]
    struct MoreJunk(#[allow(dead_code)] u8);

    let mut world = World::new();
    let ids: Vec<_> = (0..9)
        .map(|i| {
            if i % 2 == 0 {
                world.spawn(Junk(i)).id()
            } else {
                world.spawn((Junk(i), MoreJunk(1))).id()
            }
        })
        .collect();
    for id in ids.into_iter().step_by(2) {
        world.despawn(id);
    }
    let spawned = spawn::spawn_all_into(world, lump, &Models, &Gametype::free_for_all());
    MapEntities {
        world: spawned.world,
        warnings: spawned.warnings,
        unsupported: spawned.unsupported,
    }
}

fn run(map: &mut MapEntities, ticks: i64) {
    map.activate(1, 8);
    for tick in 1..=ticks {
        let time = tick * 8;
        map.think(time);
        map.step_movers(&Models, &Void, time, None);
    }
}

#[test]
fn identical_runs_have_identical_hashes_and_state_changes_it() {
    let lump = lump();
    let mut a = MapEntities::spawn(&lump, &Models);
    let mut b = MapEntities::spawn(&lump, &Models);
    assert_eq!(a.state_hash(), b.state_hash());
    let initial = a.state_hash();
    run(&mut a, 50);
    run(&mut b, 50);
    assert_ne!(a.state_hash(), initial);
    assert_eq!(a.state_hash(), b.state_hash());
    run(&mut b, 80);
    assert_ne!(a.state_hash(), b.state_hash());
}

#[test]
fn hash_ignores_spawn_layout_and_ecs_entity_ids() {
    let lump = lump();
    let mut plain = MapEntities::spawn(&lump, &Models);
    let mut other = churned(&lump);
    assert_eq!(plain.state_hash(), other.state_hash());
    run(&mut plain, 120);
    run(&mut other, 120);
    assert_eq!(plain.state_hash(), other.state_hash());
}

#[test]
fn every_clock_write_keeps_tick_in_step_with_level_time() {
    let mut map = MapEntities::spawn(&lump(), &Models);
    assert_eq!(map.tick(), 0);
    map.think(80);
    assert_eq!(map.tick(), 10);
    map.step_movers(&Models, &Void, 800, None);
    assert_eq!(map.tick(), 100);
    map.activate(1, 808);
    assert_eq!(map.tick(), 101);
}

#[test]
fn rng_streams_are_seeded_per_tick_and_purpose() {
    let mut map = MapEntities::spawn(&lump(), &Models);
    map.set_seed(7);
    map.think(8);
    let first = map.rng_stream(1).next_u64();
    assert_eq!(
        first,
        map.rng_stream(1).next_u64(),
        "pure function of state"
    );
    assert_ne!(
        first,
        map.rng_stream(2).next_u64(),
        "purposes are independent"
    );
    map.think(16);
    assert_ne!(first, map.rng_stream(1).next_u64(), "ticks are independent");
    map.set_seed(8);
    map.think(8);
    assert_ne!(first, map.rng_stream(1).next_u64(), "seeds are independent");
    map.set_seed(7);
    assert_eq!(first, map.rng_stream(1).next_u64());
}
