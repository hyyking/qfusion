//! The gametype is not tied to wbomb1: it plays on a hand-made map with a different
//! layout (one site, no capture trigger, no explosion targets besides one blast).

mod common;

use std::sync::Arc;

use common::Harness;
use qf_engine::content::bsp::entities::Entity;
use qf_engine::game::{BOMB_SCRIPT, GameHost, Team};
use qf_engine::simulation::pmove::{PmTrace, PmoveWorld, Vec3};

/// Infinite floor at z = 0.
struct Floor;

impl PmoveWorld for Floor {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, _mask: u32) -> PmTrace {
        let _ = maxs;
        let (from, to) = (start[2] + mins[2], end[2] + mins[2]);
        let mut trace = PmTrace::miss(end);
        if from < 0.0 {
            trace.start_solid = true;
            trace.fraction = 0.0;
            trace.end_position = start;
        } else if to < 0.0 {
            let fraction = from / (from - to);
            trace.fraction = fraction;
            trace.plane_normal = [0.0, 0.0, 1.0];
            trace.end_position = std::array::from_fn(|i| start[i] + (end[i] - start[i]) * fraction);
        }
        trace
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        u32::from(point[2] < 0.0)
    }
}

fn entity(pairs: &[(&str, &str)]) -> Entity {
    Entity {
        properties: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn host(entities: &[Entity]) -> GameHost {
    GameHost::new(BOMB_SCRIPT, Arc::new(Floor), entities, &[], None, 5).unwrap()
}

fn map() -> Vec<Entity> {
    vec![
        entity(&[
            ("classname", "team_CTF_betaspawn"),
            ("origin", "0 0 24"),
            ("angle", "90"),
        ]),
        entity(&[
            ("classname", "team_CTF_alphaspawn"),
            ("origin", "1000 0 24"),
        ]),
        entity(&[
            ("classname", "misc_capture_area_indicator"),
            ("origin", "200 0 100"),
            ("target", "boom"),
        ]),
        entity(&[
            ("classname", "target_explosion"),
            ("targetname", "boom"),
            ("origin", "200 0 50"),
            ("radius", "300"),
            ("dmg", "150"),
            ("delay", "0.2"),
        ]),
    ]
}

fn harness(entities: &[Entity]) -> Harness {
    let mut host = host(entities);
    host.add_player(Team::Alpha).unwrap();
    host.add_player(Team::Beta).unwrap();
    host.start_match().unwrap();
    Harness::new(host, 2)
}

#[test]
fn plants_by_proximity_and_the_bomb_wins_the_round() {
    let mut h = harness(&map());
    h.start_round();
    let spawn = h.spawn_of(0);
    assert_eq!(
        spawn,
        [0.0, 0.0, 24.0],
        "attackers use the betaspawn entities"
    );
    h.held = vec![Some([300.0, 0.0, 24.0]), Some(h.spawn_of(1))];
    h.run_until(2_000, |h| h.bomb_state() == "planting");
    h.run_until(6_000, |h| h.bomb_state() == "armed");
    h.run_until(40_000, |h| h.bomb_state() == "exploded");
    assert_eq!(h.round_won(), Some((0, "bomb_exploded".into())));
    assert!(!h.host.player(0).alive, "the planter stood in the blast");
    assert!(
        !h.host.player(1).alive,
        "an open-air blast reaches the defender too"
    );
}

#[test]
fn rejects_maps_missing_the_objective() {
    let mut entities = map();
    entities.retain(|e| e.get("classname") != Some("misc_capture_area_indicator"));
    let result = GameHost::new(BOMB_SCRIPT, Arc::new(Floor), &entities, &[], None, 5);
    let error = result.expect_err("no bomb site").to_string();
    assert!(error.contains("misc_capture_area_indicator"), "{error}");
}

#[test]
fn moving_too_fast_prevents_planting() {
    let mut h = harness(&map());
    h.start_round();
    for _ in 0..200 {
        h.host
            .set_player_motion(0, [300.0, 0.0, 24.0], [500.0, 0.0, 0.0], 0.0, 0.0);
        h.host.tick().unwrap();
        h.events.extend(h.host.take_events());
    }
    assert_eq!(h.bomb_state(), "carried");
}

#[test]
fn drop_command_tosses_the_bomb_and_a_teammate_can_pick_it_up_after_the_delay() {
    let mut h = harness(&map());
    h.start_round();
    h.held = vec![Some([-900.0, 0.0, 24.0]), Some([1000.0, 0.0, 24.0])];
    h.tick();
    assert!(h.host.command(0, "drop", "").unwrap());
    h.run_until(2_000, |h| h.bomb_state() == "dropped");
    let at = h.find("bomb_dropped")[0].vec3("origin").unwrap();
    h.held[0] = Some([at[0] + 183.0, at[1], 24.0]);
    h.run_until(3_000, |h| h.bomb_state() == "carried");
    assert_eq!(h.find("bomb_picked_up").len(), 1);
}

#[test]
fn team_damage_in_spawn_protection_is_reflected() {
    let mut host = host(&map());
    host.add_player(Team::Alpha).unwrap();
    host.add_player(Team::Alpha).unwrap();
    host.add_player(Team::Beta).unwrap();
    host.start_match().unwrap();
    let mut h = Harness::new(host, 3);
    h.start_round();
    h.host.damage(1, Some(0), 40.0).unwrap();
    h.tick();
    assert_eq!(h.host.player(1).health, 100.0, "victim untouched");
    assert!(h.host.player(0).health < 100.0, "attacker punished");
    assert_eq!(h.find("punished").len(), 1);
}

#[test]
fn loadout_commands_validate_choices() {
    let mut h = harness(&map());
    assert!(h.host.command(0, "primary", "lg eb").unwrap());
    assert!(!h.host.command(0, "primary", "lg lg").unwrap());
    assert!(!h.host.command(0, "primary", "lg pg").unwrap());
    assert!(h.host.command(0, "secondary", "rg").unwrap());
    assert!(!h.host.command(0, "secondary", "rl").unwrap());
    let weapons = h.host.player(0).weapons;
    let names: Vec<&str> = weapons.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["gunblade", "lg", "eb", "rg"]);
    assert_eq!(weapons[1].1, 180);
}

#[test]
fn script_swap_rule_matches_legacy() {
    let mut engine = rhai::Engine::new();
    engine.set_max_expr_depths(64, 64);
    let ast = engine.compile(BOMB_SCRIPT).unwrap();
    let swap = |round: i64, limit: i64| -> bool {
        engine
            .call_fn(&mut rhai::Scope::new(), &ast, "swap_roles", (round, limit))
            .unwrap()
    };
    assert!(!swap(0, 16));
    assert!(swap(15, 16));
    assert!(!swap(16, 16));
    assert!(swap(30, 16));
    assert!(swap(0, 0));
    assert!(swap(0, 1));
}
