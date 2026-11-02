//! The ECS `Movement` system against the direct `pmove` + touch-hook path.

use super::*;
use crate::simulation::pmove::{CONTENTS_SOLID, PmTrace, PmoveWorld};

struct Models;

impl BrushModels for Models {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        match index {
            0 => Some(([0.0; 3], [0.0; 3])),
            1 => Some(([-32.0, -32.0, -8.0], [32.0, 32.0, 8.0])),
            2 => Some(([200.0, -64.0, 0.0], [232.0, 64.0, 128.0])),
            _ => None,
        }
    }

    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (lo, hi) = self.model_bounds(index).unwrap();
        (0..3).all(|axis| {
            mins[axis] <= hi[axis] + origin[axis] && maxs[axis] >= lo[axis] + origin[axis]
        })
    }
}

/// Flat floor at z = 0.
struct Floor;

impl PmoveWorld for Floor {
    fn trace(&self, start: Vec3, mins: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
        let (from, to) = (start[2] + mins[2], end[2] + mins[2]);
        if from < 0.0 || to >= 0.0 {
            return PmTrace::miss(end);
        }
        let fraction = from / (from - to);
        let mut hit = PmTrace::miss(end);
        hit.fraction = fraction;
        hit.end_position = std::array::from_fn(|i| start[i] + (end[i] - start[i]) * fraction);
        hit.plane_normal = [0.0, 0.0, 1.0];
        hit.contents = CONTENTS_SOLID;
        hit
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        if point[2] < 0.0 { CONTENTS_SOLID } else { 0 }
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

fn map() -> MapEntities {
    let lump = [
        ent(&[("classname", "worldspawn"), ("gravity", "800")]),
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("target", "t"),
        ]),
        ent(&[
            ("classname", "target_position"),
            ("targetname", "t"),
            ("origin", "150 0 96"),
        ]),
        ent(&[
            ("classname", "func_door"),
            ("model", "*2"),
            ("speed", "100"),
        ]),
    ];
    MapEntities::spawn(&lump, &Models)
}

fn command(tick: usize) -> WalkCommand {
    WalkCommand {
        forward: 1.0,
        right: if tick % 90 < 20 { 0.5 } else { 0.0 },
        up: if tick % 70 == 5 { 1.0 } else { 0.0 },
        special: tick % 110 == 40,
        view_angles: [0.0, (tick as f32 * 0.7).sin() * 25.0, 0.0],
    }
}

fn walker() -> WalkPlayer {
    let mut player = WalkPlayer::from_noclip(&crate::simulation::player::NoclipPlayer {
        position: [-140.0, 0.0, 0.0],
        view_angles: [0.0; 3],
    });
    player.state.origin = [-140.0, 0.0, 24.0];
    player
}

#[test]
fn ecs_movement_is_bit_identical_to_the_direct_path() {
    let (mut direct, mut direct_map) = (walker(), map());
    let mut ecs = map();
    ecs.set_player(0, walker());
    let mut touched_a_trigger = false;

    for tick in 0..600 {
        let time = (tick as i64 + 1) * 8;
        let command = command(tick);

        let solid = MoverWorld::new(&Floor, &Models, direct_map.mover_poses());
        let mut touch = direct_map.player_touch(&Models, time);
        let outcome = direct
            .step_with_outcome(&solid, command, Some(&mut touch))
            .unwrap();
        direct_map.player_touched(&direct.touched, time);
        direct_map.step_movers(
            &Models,
            &Floor,
            time,
            Some(PushedPlayer {
                state: &mut direct.state,
                ground_entity: direct.ground_entity,
            }),
        );
        let direct_events = direct_map.drain_events();

        ecs.push_input(0, command);
        ecs.tick_players(&Floor, &Models, time).unwrap();
        let moved = ecs.player(0).unwrap().clone();
        let ecs_events = ecs.drain_events();

        assert_eq!(moved.state, direct.state, "state at tick {tick}");
        assert_eq!(moved.events, direct.events, "events at tick {tick}");
        assert_eq!(moved.ground_entity, direct.ground_entity);
        assert_eq!(moved.touched, direct.touched);
        assert_eq!(moved.view_angles, direct.view_angles);
        assert_eq!(
            ecs.last_outcome(0),
            Some(&outcome),
            "outcome at tick {tick}"
        );
        assert_eq!(ecs_events, direct_events, "trigger events at tick {tick}");
        assert_eq!(ecs.mover_poses(), direct_map.mover_poses());
        touched_a_trigger |= !moved.events.is_empty();
    }
    assert!(touched_a_trigger, "the scene must exercise the jump pad");
    assert_eq!(ecs.tick(), 600);
}

#[test]
fn players_are_visited_in_slot_order_and_hash_ignores_insertion_order() {
    let mut a = map();
    a.set_player(2, walker());
    a.set_player(0, walker());
    let mut b = map();
    b.set_player(0, walker());
    b.set_player(2, walker());
    for tick in 0..50 {
        for map in [&mut a, &mut b] {
            map.push_input(0, command(tick));
            map.push_input(2, command(tick + 3));
            map.tick_players(&Floor, &Models, (tick as i64 + 1) * 8)
                .unwrap();
        }
    }
    assert_eq!(a.state_hash(), b.state_hash());
    let (first, second) = (a.player(0).unwrap().state, a.player(2).unwrap().state);
    assert_ne!(first, second);
    assert!(a.remove_player(2));
    assert!(a.player(2).is_none());
}

#[test]
fn non_finite_input_is_reported_without_moving() {
    let mut map = map();
    map.set_player(0, walker());
    let before = map.player(0).unwrap().state;
    map.push_input(
        0,
        WalkCommand {
            forward: f32::NAN,
            ..WalkCommand::default()
        },
    );
    assert_eq!(
        map.tick_players(&Floor, &Models, 8),
        Err(StepError::NonFiniteInput)
    );
    assert_eq!(map.player(0).unwrap().state, before);
}
