//! Map entities driven through the fixed-tick `ModeSimulation`, plus the real wbomb1 lump.

use std::time::Duration;

use qf_engine::content::bsp::entities::Entity;
use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use qf_engine::simulation::entities::{BrushModels, MapEntities, TriggerEvent};
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::pmove::{PmTrace, PmoveWorld};
use qf_engine::simulation::walk::{ModeSimulation, MovementMode, WalkCommand};

struct Boxes(Vec<([f32; 3], [f32; 3])>);

impl BrushModels for Boxes {
    fn model_bounds(&self, index: usize) -> Option<([f32; 3], [f32; 3])> {
        self.0.get(index).copied()
    }

    fn box_touches_model(
        &self,
        index: usize,
        origin: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
    ) -> bool {
        let (lo, hi) = self.0[index];
        (0..3).all(|a| mins[a] <= hi[a] + origin[a] && maxs[a] >= lo[a] + origin[a])
    }
}

struct EmptyWorld;

impl PmoveWorld for EmptyWorld {
    fn trace(&self, _: [f32; 3], _: [f32; 3], _: [f32; 3], end: [f32; 3], _: u32) -> PmTrace {
        PmTrace::miss(end)
    }

    fn point_contents(&self, _: [f32; 3]) -> u32 {
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

fn walking_sim(position: [f32; 3], entities: MapEntities) -> ModeSimulation {
    let mut sim = ModeSimulation::new(NoclipPlayer {
        position,
        view_angles: [0.0; 3],
    });
    sim.toggle_mode();
    sim.set_entities(entities);
    sim
}

fn state_of(sim: &ModeSimulation) -> ([f32; 3], [f32; 3]) {
    match sim.mode() {
        MovementMode::Walk(player) => (player.state.origin, player.state.velocity),
        MovementMode::Noclip(_) => unreachable!(),
    }
}

fn pad_scene() -> (Boxes, Vec<Entity>) {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = vec![
        ent(&[("classname", "worldspawn"), ("gravity", "800")]),
        ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("target", "t1"),
        ]),
        ent(&[
            ("classname", "target_position"),
            ("targetname", "t1"),
            ("origin", "300 0 128"),
        ]),
    ];
    (boxes, entities)
}

#[test]
fn jump_pad_launch_is_independent_of_frame_grouping() {
    let (boxes, entities) = pad_scene();
    let run = |frames: &[u64]| {
        let map = MapEntities::spawn(&entities, &boxes);
        let mut sim = walking_sim([0.0, 0.0, 24.0], map);
        let mut events = Vec::new();
        for ms in frames {
            sim.advance_frame_with_entities(
                &EmptyWorld,
                &boxes,
                Duration::from_millis(*ms),
                WalkCommand::default(),
            )
            .unwrap();
            events.extend(sim.drain_trigger_events());
        }
        (state_of(&sim), events)
    };
    let whole = run(&[400]);
    let split = run(&[8; 50]);
    assert_eq!(whole, split);

    let (_, events) = whole;
    let [TriggerEvent::JumpPad { velocity, .. }] = events[..] else {
        panic!("expected exactly one jump pad event, got {events:?}");
    };
    // Legacy: time = sqrt(height / (0.5 * gravity)); height = 128 - 0 (pad centre).
    let time = (128.0f64 / 400.0).sqrt() as f32;
    assert!((velocity[2] - time * 800.0).abs() < 1e-3);
    assert!((velocity[0] - 300.0 / time).abs() < 1e-2);
}

#[test]
fn teleporter_moves_and_turns_the_walking_player() {
    let boxes = Boxes(vec![
        ([0.0; 3], [0.0; 3]),
        ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
    ]);
    let entities = vec![
        ent(&[("classname", "worldspawn")]),
        ent(&[
            ("classname", "trigger_teleport"),
            ("model", "*1"),
            ("target", "dest"),
        ]),
        ent(&[
            ("classname", "misc_teleporter_dest"),
            ("targetname", "dest"),
            ("origin", "1000 500 100"),
            ("angle", "90"),
        ]),
    ];
    let map = MapEntities::spawn(&entities, &boxes);
    let mut sim = walking_sim([0.0, 0.0, 24.0], map);
    sim.advance_frame_with_entities(
        &EmptyWorld,
        &boxes,
        Duration::from_millis(8),
        WalkCommand::default(),
    )
    .unwrap();
    let events = sim.drain_trigger_events();
    assert!(matches!(events[..], [TriggerEvent::Teleport { .. }]));
    let (origin, _) = state_of(&sim);
    assert!((origin[0] - 1000.0).abs() < 1.0 && (origin[1] - 500.0).abs() < 1.0);
    let (_, angles) = sim.mode().view();
    assert!((angles[1] - 90.0).abs() < 0.1, "yaw {angles:?}");
}

#[test]
fn all_official_maps_spawn_without_warnings() {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let Ok(dir) = std::fs::read_dir(&base) else {
        eprintln!("skipping: {base} unavailable");
        return;
    };
    let mut union = std::collections::BTreeMap::<String, usize>::new();
    let mut maps = 0;
    for file in dir.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let Some(map_name) = name
            .strip_prefix("map_")
            .and_then(|n| n.strip_suffix(".pk3"))
        else {
            continue;
        };
        let Ok(data) = read_pk3_entry(&file.path(), &format!("maps/{map_name}.bsp")) else {
            continue;
        };
        let Ok(bsp) = Bsp::parse(&data) else { continue };
        let map = MapEntities::spawn(&bsp.parsed_entities, &bsp);
        assert!(
            map.warnings().is_empty(),
            "{map_name}: {:?}",
            map.warnings()
        );
        for (classname, count) in map.unsupported_classnames() {
            *union.entry(classname.clone()).or_default() += count;
        }
        maps += 1;
    }
    eprintln!("{maps} maps; unsupported union: {union:#?}");
    for classname in qf_engine::simulation::entities::SUPPORTED_MOVERS {
        assert!(
            !union.contains_key(*classname),
            "{classname} is unsupported"
        );
    }
}

/// Walks from a spawn on every official map that has movers, activating them
/// all, and checks that nothing goes non-finite.
#[test]
fn all_official_maps_with_movers_run_headlessly() {
    use qf_engine::simulation::bsp_world::BspWorld;
    use qf_engine::simulation::entities::SUPPORTED_MOVERS;
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let Ok(dir) = std::fs::read_dir(&base) else {
        eprintln!("skipping: {base} unavailable");
        return;
    };
    let mut with_movers = 0;
    for file in dir.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let Some(map_name) = name
            .strip_prefix("map_")
            .and_then(|n| n.strip_suffix(".pk3"))
        else {
            continue;
        };
        let Ok(data) = read_pk3_entry(&file.path(), &format!("maps/{map_name}.bsp")) else {
            continue;
        };
        let Ok(bsp) = Bsp::parse(&data) else { continue };
        let map = MapEntities::spawn(&bsp.parsed_entities, &bsp);
        let movers: Vec<usize> = SUPPORTED_MOVERS
            .iter()
            .flat_map(|classname| map.triggers_of(classname))
            .collect();
        if movers.is_empty() {
            continue;
        }
        with_movers += 1;
        let spawn = bsp
            .parsed_entities
            .iter()
            .find(|entity| {
                entity
                    .properties
                    .iter()
                    .any(|(k, v)| k == "classname" && v.starts_with("info_player_"))
            })
            .and_then(|entity| {
                entity
                    .properties
                    .iter()
                    .find(|(k, _)| k == "origin")
                    .map(|(_, v)| v.clone())
            })
            .and_then(|origin| {
                let v: Vec<f32> = origin
                    .split_ascii_whitespace()
                    .filter_map(|c| c.parse().ok())
                    .collect();
                (v.len() == 3).then(|| [v[0], v[1], v[2] + 8.0])
            })
            .unwrap_or_else(|| panic!("{map_name}: no spawn"));
        let world = BspWorld::new(&bsp);
        let mut sim = walking_sim(spawn, map);
        let mut walked = 0.0f32;
        for tick in 0..1500 {
            if tick == 125 {
                let time = 125 * 8;
                let entities = sim.entities_mut().unwrap();
                for &lump in &movers {
                    entities.activate(lump, time);
                }
            }
            let command = WalkCommand {
                forward: 1.0,
                view_angles: [0.0, (tick / 100) as f32 * 40.0, 0.0],
                ..WalkCommand::default()
            };
            sim.advance_frame_with_entities(&world, &bsp, Duration::from_millis(8), command)
                .unwrap();
            let (origin, velocity) = state_of(&sim);
            assert!(
                origin.iter().chain(&velocity).all(|v| v.is_finite()),
                "{map_name} tick {tick}: {origin:?} {velocity:?}"
            );
            for pose in sim.entities().unwrap().mover_poses() {
                assert!(
                    pose.origin
                        .iter()
                        .chain(&pose.angles)
                        .all(|v| v.is_finite()),
                    "{map_name} tick {tick}: {pose:?}"
                );
            }
            walked = walked.max(
                (0..2)
                    .map(|a| (origin[a] - spawn[a]).abs())
                    .fold(0.0, f32::max),
            );
        }
        assert!(walked > 16.0, "{map_name}: the player never left the spawn");
    }
    eprintln!("{with_movers} maps with movers ran");
}

#[test]
fn real_wbomb1_pads_launch_a_walking_player() {
    use qf_engine::simulation::bsp_world::BspWorld;
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = std::path::Path::new(&base).join("map_wbomb1.pk3");
    let Ok(data) = read_pk3_entry(&pk3, "maps/wbomb1.bsp") else {
        eprintln!("skipping: {} unavailable", pk3.display());
        return;
    };
    let bsp = Bsp::parse(&data).unwrap();
    let world = BspWorld::new(&bsp);
    let mut checked = 0;
    for entity in &bsp.parsed_entities {
        let get = |key: &str| {
            entity
                .properties
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
        };
        if get("classname") != Some("trigger_push") {
            continue;
        }
        let index: usize = get("model").unwrap()[1..].parse().unwrap();
        let (lo, hi) = bsp.model_bounds(index).unwrap();
        let start = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, hi[2] + 24.0];
        let map = MapEntities::spawn(&bsp.parsed_entities, &bsp);
        let mut sim = walking_sim(start, map);
        let mut events = Vec::new();
        for _ in 0..250 {
            sim.advance_frame_with_entities(
                &world,
                &bsp,
                Duration::from_millis(8),
                WalkCommand::default(),
            )
            .unwrap();
            events.extend(sim.drain_trigger_events());
        }
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TriggerEvent::JumpPad { .. })),
            "pad {lo:?}..{hi:?} never fired: {events:?}"
        );
        checked += 1;
    }
    assert_eq!(checked, 2);
}

#[test]
fn real_wbomb1_entities_spawn() {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = std::path::Path::new(&base).join("map_wbomb1.pk3");
    let Ok(data) = read_pk3_entry(&pk3, "maps/wbomb1.bsp") else {
        eprintln!("skipping: {} unavailable", pk3.display());
        return;
    };
    let bsp = Bsp::parse(&data).unwrap();
    let map = MapEntities::spawn(&bsp.parsed_entities, &bsp);
    assert!(map.warnings().is_empty(), "{:?}", map.warnings());
    let pads = map.triggers_of("trigger_push");
    assert_eq!(pads.len(), 2);
    for pad in pads {
        let velocity = map.push_velocity(pad).unwrap();
        assert!(velocity.iter().all(|c| c.is_finite()));
        assert!(velocity[2] > 0.0, "pad {pad} velocity {velocity:?}");
    }
    assert!(!map.unsupported_classnames().contains_key("trigger_push"));
    assert_eq!(
        map.unsupported_classnames().get("target_location"),
        Some(&78)
    );
}
