//! Deterministic tests of the brush movers against box worlds.

use super::*;
use crate::simulation::pmove::{
    CONTENTS_SOLID, PmTrace, PmoveCommand, PmoveRules, PmoveWorld, pmove_with_touch,
};

struct Models(Vec<(Vec3, Vec3)>);

impl BrushModels for Models {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        self.0.get(index).copied()
    }

    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (lo, hi) = self.0[index];
        (0..3).all(|axis| {
            mins[axis] <= hi[axis] + origin[axis] && maxs[axis] >= lo[axis] + origin[axis]
        })
    }
}

/// Static solid boxes; hits report entity 0.
struct Solids(Vec<(Vec3, Vec3)>);

impl PmoveWorld for Solids {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, _: u32) -> PmTrace {
        let mut best = PmTrace::miss(end);
        for &(lo, hi) in &self.0 {
            let trace = sweep_box(
                std::array::from_fn(|axis| lo[axis] - maxs[axis]),
                std::array::from_fn(|axis| hi[axis] - mins[axis]),
                start,
                end,
            );
            if trace.fraction < best.fraction {
                best = PmTrace {
                    fraction: trace.fraction,
                    end_position: trace.end_position,
                    plane_normal: trace.plane.map_or([0.0; 3], |plane| plane.normal),
                    surface_flags: 0,
                    contents: CONTENTS_SOLID,
                    entity: 0,
                    start_solid: trace.start_solid,
                    all_solid: trace.all_solid,
                };
            }
        }
        best
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        let inside = self
            .0
            .iter()
            .any(|&(lo, hi)| (0..3).all(|axis| point[axis] > lo[axis] && point[axis] < hi[axis]));
        if inside { CONTENTS_SOLID } else { 0 }
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

/// Player and map driven in the same order as `ModeSimulation`.
struct Rig {
    map: MapEntities,
    models: Models,
    solids: Solids,
    state: PmoveState,
    ground: Option<i32>,
    time: i64,
    events: Vec<TriggerEvent>,
}

impl Rig {
    fn new(
        entities: &[Entity],
        models: Vec<(Vec3, Vec3)>,
        solids: Vec<(Vec3, Vec3)>,
        origin: Vec3,
    ) -> Self {
        let mut all = vec![ent(&[("classname", "worldspawn")])];
        all.extend(entities.iter().cloned());
        let models = Models(models);
        let map = MapEntities::spawn(&all, &models);
        assert!(map.warnings().is_empty(), "{:?}", map.warnings());
        Self {
            map,
            models,
            solids: Solids(solids),
            state: PmoveState::new(origin),
            ground: None,
            time: 0,
            events: Vec::new(),
        }
    }

    fn tick(&mut self) {
        self.time += 8;
        let world = MoverWorld::new(&self.solids, &self.models, self.map.mover_poses());
        let command = PmoveCommand {
            msec: 8,
            ..PmoveCommand::default()
        };
        let mut touch = self.map.player_touch(&self.models, self.time);
        let result = pmove_with_touch(
            &world,
            &PmoveRules::default(),
            &mut self.state,
            &command,
            &mut touch,
        );
        self.ground = result.ground_entity;
        self.map.player_touched(&result.touch_entities, self.time);
        self.map.step_movers(
            &self.models,
            &self.solids,
            self.time,
            Some(PushedPlayer {
                state: &mut self.state,
                ground_entity: self.ground,
            }),
        );
        self.events.extend(self.map.drain_events());
    }

    fn mover(&self, lump: usize) -> MoverPose {
        self.map
            .mover_poses()
            .into_iter()
            .find(|pose| pose.entity == lump)
            .unwrap()
    }
}

const FLOOR: (Vec3, Vec3) = ([-4096.0, -4096.0, -64.0], [4096.0, 4096.0, 0.0]);

#[test]
fn pose_maps_model_points_into_world_space() {
    let pose = MoverPose {
        entity: 1,
        model: 1,
        origin: [10.0, 0.0, 0.0],
        angles: [0.0, 90.0, 0.0],
    };
    let world = pose.local_to_world([1.0, 0.0, 0.0]);
    assert!((world[0] - 10.0).abs() < 1e-5 && (world[1] - 1.0).abs() < 1e-5);
}

#[test]
fn door_opens_and_closes_with_exact_timing() {
    let door = ent(&[
        ("classname", "func_door"),
        ("model", "*1"),
        ("targetname", "d"),
        ("speed", "100"),
        ("wait", "2"),
    ]);
    // 64 units thick along +x (angle 0) minus the 8 unit lip: 56 units of travel.
    let mut rig = Rig::new(
        &[door],
        vec![([0.0; 3], [0.0; 3]), ([0.0, 0.0, 0.0], [64.0, 64.0, 128.0])],
        vec![FLOOR],
        [-500.0, 0.0, 24.0],
    );
    assert!(rig.map.activate(1, 0));
    let x = |rig: &Rig| rig.mover(1).origin[0];

    rig.tick();
    assert!(
        (x(&rig) - 0.8).abs() < 1e-4,
        "first tick moves 8 ms of travel"
    );
    while rig.time < 552 {
        rig.tick();
    }
    assert!(x(&rig) < 56.0);
    rig.tick();
    assert_eq!(rig.time, 560);
    assert_eq!(x(&rig), 56.0, "snaps to the end after 560 ms");

    // Open until the 2 s wait expires, then closes with the same timing.
    while rig.time < 2560 {
        rig.tick();
    }
    assert_eq!(x(&rig), 56.0);
    rig.tick();
    assert!((x(&rig) - 55.2).abs() < 1e-4, "closing starts at 2568");
    while rig.time < 3120 {
        rig.tick();
    }
    assert_eq!(x(&rig), 0.0);
    assert!(
        rig.events
            .iter()
            .all(|event| !matches!(event, TriggerEvent::Crush { .. }))
    );
}

#[test]
fn door_trigger_field_opens_a_door_without_targetname() {
    let door = ent(&[("classname", "func_door"), ("model", "*1")]);
    let mut rig = Rig::new(
        &[door],
        vec![([0.0; 3], [0.0; 3]), ([0.0, 0.0, 0.0], [64.0, 64.0, 128.0])],
        vec![FLOOR],
        [32.0, 120.0, 24.0],
    );
    for _ in 0..40 {
        rig.tick();
    }
    assert!(rig.mover(1).origin[0] > 0.0, "the 80 unit field fired");
}

#[test]
fn blocked_door_crushes_and_reverses() {
    let door = ent(&[
        ("classname", "func_door"),
        ("model", "*1"),
        ("targetname", "d"),
        ("speed", "100"),
    ]);
    // The player stands between the door and a wall and cannot be pushed away.
    let wall = ([100.0, -64.0, 0.0], [164.0, 64.0, 256.0]);
    let mut rig = Rig::new(
        &[door],
        vec![
            ([0.0; 3], [0.0; 3]),
            ([0.0, -32.0, 0.0], [64.0, 32.0, 128.0]),
        ],
        vec![FLOOR, wall],
        [84.0, 0.0, 24.0],
    );
    rig.map.activate(1, 0);
    for _ in 0..30 {
        rig.tick();
    }
    let crushes: Vec<_> = rig
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                TriggerEvent::Crush {
                    entity: 1,
                    damage: 2
                }
            )
        })
        .collect();
    assert!(!crushes.is_empty(), "{:?}", rig.events);
    assert_eq!(
        rig.state.origin[0], 84.0,
        "the player is not squeezed into the wall"
    );
    // It reversed and keeps closing (no `wait` stall, since the player blocks it).
    assert!(rig.mover(1).origin[0] < 5.0);
}

#[test]
fn player_standing_on_a_plat_rides_it_up() {
    let plat = ent(&[
        ("classname", "func_plat"),
        ("model", "*1"),
        ("height", "100"),
    ]);
    let floor = ([-4096.0, -4096.0, -364.0], [4096.0, 4096.0, -300.0]);
    let mut rig = Rig::new(
        &[plat],
        vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -16.0], [64.0, 64.0, 0.0]),
        ],
        vec![floor],
        [0.0, 0.0, -100.0 + 24.0],
    );
    assert_eq!(
        rig.mover(1).origin[2],
        -100.0,
        "idle plats wait at the bottom"
    );
    let start = rig.state.origin[2];
    let mut lowest = start;
    for _ in 0..80 {
        rig.tick();
        lowest = lowest.min(rig.state.origin[2]);
    }
    assert!(lowest > start - 1.0, "never fell through the plat");
    assert_eq!(rig.mover(1).origin[2], 0.0);
    assert!(
        (rig.state.origin[2] - 24.0).abs() < 0.5,
        "rode up with the plat: {}",
        rig.state.origin[2]
    );
    assert_eq!(rig.ground, Some(1));
}

#[test]
fn rotating_mover_carries_the_player_around_its_axis() {
    let spinner = ent(&[
        ("classname", "func_rotating"),
        ("model", "*1"),
        ("speed", "90"),
    ]);
    let mut rig = Rig::new(
        &[spinner],
        vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, 0.0], [64.0, 64.0, 32.0]),
        ],
        vec![([-4096.0, -4096.0, -64.0], [4096.0, 4096.0, -32.0])],
        [48.0, 0.0, 32.0 + 24.0],
    );
    for _ in 0..125 {
        rig.tick();
    }
    assert!((rig.mover(1).angles[1] - 90.0).abs() < 0.01);
    let [x, y, _] = rig.state.origin;
    assert!(x.abs() < 1.5 && (y - 48.0).abs() < 1.5, "({x}, {y})");
}

#[test]
fn movers_are_deterministic() {
    let run = || {
        let door = ent(&[("classname", "func_door"), ("model", "*1")]);
        let mut rig = Rig::new(
            &[door],
            vec![([0.0; 3], [0.0; 3]), ([0.0, 0.0, 0.0], [64.0, 64.0, 128.0])],
            vec![FLOOR],
            [32.0, 120.0, 24.0],
        );
        (0..400)
            .map(|_| {
                rig.tick();
                (rig.mover(1).origin, rig.state.origin)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(run(), run());
}
