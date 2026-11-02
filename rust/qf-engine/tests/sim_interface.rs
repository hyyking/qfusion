//! `Sim` against the `ModeSimulation` adapter on the same usercmd stream.

use qf_engine::content::bsp::entities::Entity;
use qf_engine::simulation::entities::{BrushModels, MapEntities};
use qf_engine::simulation::player::{NoclipPlayer, TICK_DURATION};
use qf_engine::simulation::pmove::{CONTENTS_SOLID, PmTrace, PmoveWorld, Vec3};
use qf_engine::simulation::sim::{Sim, SimEvent};
use qf_engine::simulation::walk::{ModeSimulation, MovementMode, WalkCommand, WalkPlayer};

struct Models;

impl BrushModels for Models {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        match index {
            0 => Some(([0.0; 3], [0.0; 3])),
            1 => Some(([-32.0, -32.0, -8.0], [32.0, 32.0, 8.0])),
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

fn start() -> NoclipPlayer {
    NoclipPlayer {
        position: [-140.0, 0.0, 24.0],
        view_angles: [0.0; 3],
    }
}

#[test]
fn sim_matches_the_mode_simulation_adapter() {
    let mut adapter = ModeSimulation::new(start());
    adapter.toggle_mode();
    adapter.set_entities(map());

    let mut sim = Sim::new(map(), 1);
    sim.add_player(0, WalkPlayer::from_noclip(&start()));

    let mut saw_trigger = false;
    for tick in 0..400 {
        let command = command(tick);
        adapter
            .advance_frame_with_entities(&Floor, &Models, TICK_DURATION, command)
            .unwrap();
        sim.push_input(0, command);
        sim.tick(&Floor, &Models).unwrap();

        let MovementMode::Walk(walker) = adapter.mode() else {
            unreachable!()
        };
        let view = sim.view();
        assert_eq!(view.tick, tick as u64 + 1);
        assert_eq!(view.players.len(), 1);
        assert_eq!(view.players[0].origin, walker.state.origin, "tick {tick}");
        assert_eq!(view.players[0].velocity, walker.state.velocity);
        assert_eq!(view.players[0].eye, walker.eye_position());
        assert_eq!(view.players[0].view_angles, walker.view_angles);
        assert_eq!(view.movers, adapter.entities().unwrap().mover_poses());

        let mut expected: Vec<SimEvent> = adapter
            .take_events()
            .into_iter()
            .map(|event| SimEvent::Pmove { slot: 0, event })
            .collect();
        expected.extend(
            adapter
                .drain_trigger_events()
                .into_iter()
                .map(SimEvent::from),
        );
        saw_trigger |= expected.iter().any(|e| matches!(e, SimEvent::Trigger(_)));
        assert_eq!(sim.drain_events(), expected, "events at tick {tick}");
    }
    assert!(saw_trigger, "the scene must fire the jump pad");
    assert_eq!(sim.map().tick(), adapter.entities().unwrap().tick());
}

#[test]
fn same_inputs_give_the_same_hash_and_different_inputs_do_not() {
    let run = |turn: f32| {
        let mut sim = Sim::new(map(), 9);
        sim.add_player(0, WalkPlayer::from_noclip(&start()));
        for tick in 0..120 {
            let mut command = command(tick);
            command.view_angles[1] += turn;
            sim.push_input(0, command);
            sim.tick(&Floor, &Models).unwrap();
        }
        sim.state_hash()
    };
    assert_eq!(run(0.0), run(0.0));
    assert_ne!(run(0.0), run(15.0));
}
