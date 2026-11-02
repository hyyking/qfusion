//! Weapons inside the `Sim`: usercmd buttons in, `SimEvent::Weapon` and entity state out.

use qf_engine::content::bsp::entities::Entity;
use qf_engine::game::{PlayerInput, WeaponEvent, WeaponId};
use qf_engine::simulation::entities::{BrushModels, MapEntities};
use qf_engine::simulation::player::NoclipPlayer;
use qf_engine::simulation::pmove::{CONTENTS_SOLID, PmTrace, PmoveWorld, Vec3};
use qf_engine::simulation::sim::{Sim, SimEvent};
use qf_engine::simulation::walk::{WalkCommand, WalkPlayer};

struct Models;

impl BrushModels for Models {
    fn model_bounds(&self, _: usize) -> Option<(Vec3, Vec3)> {
        None
    }

    fn box_touches_model(&self, _: usize, _: Vec3, _: Vec3, _: Vec3) -> bool {
        false
    }
}

/// An infinite floor at z = 0.
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

fn sim(weapon: WeaponId, pitch_up: f32) -> Sim {
    let lump = [Entity {
        properties: [("classname".to_owned(), "worldspawn".to_owned())]
            .into_iter()
            .collect(),
    }];
    let mut sim = Sim::new(MapEntities::spawn(&lump, &Models), 5);
    for (slot, x) in [(0, 0.0), (1, 300.0)] {
        let noclip = NoclipPlayer {
            position: [x, 0.0, 24.0],
            view_angles: [0.0; 3],
        };
        sim.add_player(slot, WalkPlayer::from_noclip(&noclip));
        sim.push_input(
            slot,
            WalkCommand {
                view_angles: [if slot == 0 { pitch_up } else { 0.0 }, 0.0, 0.0],
                ..WalkCommand::default()
            },
        );
    }
    sim.inventory_mut(0).unwrap().give_weapon(weapon, 10);
    for _ in 0..10 {
        sim.tick(&Floor, &Models).unwrap();
    }
    sim.drain_events();
    sim
}

fn fire(sim: &mut Sim, held: bool) {
    sim.push_weapon_input(
        0,
        PlayerInput {
            fire: held,
            ..PlayerInput::default()
        },
    );
}

fn weapon_events(sim: &mut Sim) -> Vec<WeaponEvent> {
    sim.drain_events()
        .into_iter()
        .filter_map(|event| match event {
            SimEvent::Weapon(event) => Some(event),
            _ => None,
        })
        .collect()
}

#[test]
fn hitscan_damages_and_pushes_the_player_in_front() {
    let mut sim = sim(WeaponId::Electrobolt, 0.0);
    fire(&mut sim, true);
    sim.tick(&Floor, &Models).unwrap();
    fire(&mut sim, false);
    let events = weapon_events(&mut sim);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, WeaponEvent::Fired { player: 0, .. }))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        WeaponEvent::Damage { victim: 1, attacker: 0, amount, .. } if *amount > 0.0
    )));
    let view = sim.view();
    assert!(
        view.players[1].velocity[0] > 0.0,
        "knockback moves the target"
    );
    assert_eq!(view.players[0].weapon, Some(WeaponId::Electrobolt));
    assert_eq!(view.players[0].ammo, 9);
}

#[test]
fn dead_players_are_not_hit() {
    let mut sim = sim(WeaponId::Electrobolt, 0.0);
    sim.set_alive(1, false);
    fire(&mut sim, true);
    sim.tick(&Floor, &Models).unwrap();
    let events = weapon_events(&mut sim);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, WeaponEvent::Damage { .. }))
    );
}

#[test]
fn rockets_are_entities_that_explode_on_the_floor() {
    let mut sim = sim(WeaponId::RocketLauncher, -60.0);
    fire(&mut sim, true);
    sim.tick(&Floor, &Models).unwrap();
    fire(&mut sim, false);
    assert_eq!(sim.view().projectiles.len(), 1);
    let mut all = weapon_events(&mut sim);
    for _ in 0..60 {
        sim.tick(&Floor, &Models).unwrap();
        all.extend(weapon_events(&mut sim));
    }
    assert!(sim.view().projectiles.is_empty());
    assert!(all.iter().any(|e| matches!(e, WeaponEvent::Impact { .. })));
}

#[test]
fn firing_replays_identically_and_changes_the_hash() {
    let run = |shoot: bool| {
        let mut sim = sim(WeaponId::Riotgun, 0.0);
        for tick in 0..60 {
            fire(&mut sim, shoot && tick % 2 == 0);
            sim.tick(&Floor, &Models).unwrap();
        }
        sim.state_hash()
    };
    assert_eq!(run(true), run(true));
    assert_ne!(run(true), run(false));
}

#[test]
fn noclip_flies_through_the_floor_and_reports_in_the_view() {
    let mut sim = sim(WeaponId::Electrobolt, 0.0);
    sim.set_noclip(0, true);
    sim.push_input(
        0,
        WalkCommand {
            up: -1.0,
            ..WalkCommand::default()
        },
    );
    for _ in 0..25 {
        sim.tick(&Floor, &Models).unwrap();
    }
    let view = sim.view();
    assert!(view.players[0].noclip && !view.players[1].noclip);
    assert!(view.players[0].origin[2] < 0.0, "noclip ignores the floor");
    sim.set_noclip(0, false);
    assert!(!sim.noclip(0));
}

fn logged_run() -> (Sim, qf_engine::simulation::sim::SimLog) {
    let mut sim = sim(WeaponId::Electrobolt, 0.0);
    sim.start_log();
    fire(&mut sim, true);
    for _ in 0..30 {
        sim.tick(&Floor, &Models).unwrap();
    }
    fire(&mut sim, false);
    sim.set_noclip(1, true);
    for _ in 0..10 {
        sim.tick(&Floor, &Models).unwrap();
    }
    let log = sim.take_log().unwrap();
    (sim, log)
}

#[test]
fn a_log_replays_to_the_same_hashes_on_a_fresh_sim() {
    use qf_engine::simulation::sim::SimReplayError;
    let (_, log) = logged_run();
    assert_eq!(log.ticks(), 40);
    // Setup (players, inventory) is not part of the log, so rebuild it the same way.
    let mut fresh = sim(WeaponId::Electrobolt, 0.0);
    let mut inputs = log.clone();
    inputs
        .ops
        .retain(|op| !matches!(op, qf_engine::simulation::sim::LogOp::Add { .. }));
    assert_eq!(inputs.replay(&mut fresh, &Floor, &Models), Ok(()));

    let mut other = sim(WeaponId::Electrobolt, 0.0);
    other.set_alive(1, false);
    assert!(matches!(
        inputs.replay(&mut other, &Floor, &Models),
        Err(SimReplayError::Diverged { .. })
    ));
}
