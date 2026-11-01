//! Walking on real BSP brush collision. These tests cover the collision adapter and
//! the walking player; the legacy trace replay (`tests/pmove_traces.rs`) covers
//! movement integration on its own plane world, so a failure here with a passing
//! replay points at BSP collision or the adapter rather than the integrator.

use std::time::Duration;

use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::walk::{ModeSimulation, MovementMode, WalkCommand};

fn le(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

/// One solid slab: x,y in [-1024, 1024], z in [-16, 0]; empty space above.
fn floor_bsp() -> Vec<u8> {
    let mut shader = vec![0u8; 72];
    shader[64..68].copy_from_slice(&0i32.to_le_bytes());
    shader[68..72].copy_from_slice(&1i32.to_le_bytes());
    let mut planes = Vec::new();
    for (normal, dist) in [
        ([1.0f32, 0.0, 0.0], 1024.0f32),
        ([-1.0, 0.0, 0.0], 1024.0),
        ([0.0, 1.0, 0.0], 1024.0),
        ([0.0, -1.0, 0.0], 1024.0),
        ([0.0, 0.0, 1.0], 0.0),
        ([0.0, 0.0, -1.0], 16.0),
    ] {
        for component in normal {
            planes.extend(component.to_le_bytes());
        }
        planes.extend(dist.to_le_bytes());
    }
    let mut node = vec![0u8; 36];
    for (offset, value) in [(0, 4), (4, -1), (8, -2)] {
        node[offset..offset + 4].copy_from_slice(&le(value));
    }
    let mut leafs = Vec::new();
    for (first_brush, brush_count) in [(0, 0), (0, 1)] {
        let mut leaf = vec![0u8; 48];
        leaf[40..44].copy_from_slice(&le(first_brush));
        leaf[44..48].copy_from_slice(&le(brush_count));
        leafs.extend(leaf);
    }
    let mut model = Vec::new();
    for f in [-1024.0f32, -1024.0, -16.0, 1024.0, 1024.0, 0.0] {
        model.extend(f.to_le_bytes());
    }
    for v in [0, 0, 0, 1] {
        model.extend(le(v));
    }
    let mut brush = Vec::new();
    for value in [0, 6, 0] {
        brush.extend(le(value));
    }
    let mut brush_sides = Vec::new();
    for plane in 0..6 {
        brush_sides.extend(le(plane));
        brush_sides.extend(le(0));
        brush_sides.extend(le(-1));
    }
    let lumps: Vec<(usize, Vec<u8>)> = vec![
        (0, b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec()),
        (1, shader),
        (2, planes),
        (3, node),
        (4, leafs),
        (6, le(0).to_vec()),
        (7, model),
        (8, brush),
        (9, brush_sides),
    ];
    let header_len = 8 + 18 * 8;
    let mut out = b"FBSP".to_vec();
    out.extend(le(1));
    let mut table = vec![(0i32, 0i32); 18];
    let mut body: Vec<u8> = Vec::new();
    for (index, data) in &lumps {
        table[*index] = ((header_len + body.len()) as i32, data.len() as i32);
        body.extend(data);
    }
    for (offset, len) in table {
        out.extend(le(offset));
        out.extend(le(len));
    }
    out.extend(body);
    out
}

fn walk(sim: &mut ModeSimulation, world: &BspWorld, ticks: u32, command: WalkCommand) {
    sim.advance_frame(world, Duration::from_millis(8 * ticks as u64), command)
        .unwrap();
}

fn origin(sim: &ModeSimulation) -> [f32; 3] {
    match sim.mode() {
        MovementMode::Walk(player) => player.state.origin,
        MovementMode::Noclip(_) => panic!("expected walking"),
    }
}

fn start() -> ModeSimulation {
    let mut sim = ModeSimulation::new(NoclipPlayer {
        position: [0.0, 0.0, 60.0],
        view_angles: [0.0; 3],
    });
    sim.toggle_mode();
    sim
}

#[test]
fn falls_lands_and_stands_on_brush_floor() {
    let bsp = Bsp::parse(&floor_bsp()).unwrap();
    let world = BspWorld::new(&bsp);
    let mut sim = start();
    walk(&mut sim, &world, 200, WalkCommand::default());
    let origin = origin(&sim);
    assert!((origin[2] - 24.0).abs() < 0.1, "standing height {origin:?}");
    let MovementMode::Walk(player) = sim.mode() else {
        unreachable!()
    };
    assert!(player.state.on_ground());
    assert_eq!(player.state.velocity, [0.0; 3]);
}

#[test]
fn runs_to_max_ground_speed_and_jumps() {
    let bsp = Bsp::parse(&floor_bsp()).unwrap();
    let world = BspWorld::new(&bsp);
    let mut sim = start();
    walk(&mut sim, &world, 100, WalkCommand::default());
    let forward = WalkCommand {
        forward: 1.0,
        ..WalkCommand::default()
    };
    walk(&mut sim, &world, 125, forward);
    let MovementMode::Walk(player) = sim.mode() else {
        unreachable!()
    };
    let speed = player.state.velocity[0].hypot(player.state.velocity[1]);
    assert!((speed - 320.0).abs() < 1.0, "ground speed {speed}");
    assert!(player.state.origin[0] > 200.0);

    walk(&mut sim, &world, 1, WalkCommand { up: 1.0, ..forward });
    let MovementMode::Walk(player) = sim.mode() else {
        unreachable!()
    };
    assert!(player.state.velocity[2] > 250.0, "jump launched");
    assert!(!player.state.on_ground());
    walk(&mut sim, &world, 200, forward);
    assert!((origin(&sim)[2] - 24.0).abs() < 0.1, "landed again");
}

#[test]
fn noclip_toggle_keeps_pose() {
    let mut sim = start();
    let walking = origin(&sim);
    sim.toggle_mode();
    let MovementMode::Noclip(player) = sim.mode() else {
        panic!("expected noclip")
    };
    assert_eq!(player.position, walking);
}

#[test]
fn real_wbomb1_walk_settles_on_ground() {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = std::path::Path::new(&base).join("map_wbomb1.pk3");
    let Ok(data) = read_pk3_entry(&pk3, "maps/wbomb1.bsp") else {
        eprintln!("skipping: {} unavailable", pk3.display());
        return;
    };
    let bsp = Bsp::parse(&data).unwrap();
    let spawn = NoclipPlayer::spawn(&bsp.parsed_entities).unwrap();
    let world = BspWorld::new(&bsp);
    let mut sim = ModeSimulation::new(spawn);
    sim.toggle_mode();
    walk(&mut sim, &world, 250, WalkCommand::default());
    let MovementMode::Walk(player) = sim.mode() else {
        unreachable!()
    };
    assert!(
        player.state.on_ground(),
        "spawn {spawn:?} -> {:?}",
        player.state.origin
    );
    assert!(player.state.origin.iter().all(|c| c.is_finite()));
}
