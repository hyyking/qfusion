//! The generic gametype host with a tiny inline script (no map, no bomb rules).

use std::sync::Arc;

use qf_engine::content::bsp::entities::Entity;
use qf_engine::game::{GameHost, Team, Value};
use qf_engine::simulation::pmove::{PmTrace, PmoveWorld, Vec3};

/// Infinite floor at z = 0.
struct Floor;

impl PmoveWorld for Floor {
    fn trace(&self, start: Vec3, mins: Vec3, _maxs: Vec3, end: Vec3, _mask: u32) -> PmTrace {
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

const SCRIPT: &str = r#"
fn init(g) {
    this.ticks = 0;
    this.spawns = [];
    for e in g.entities() {
        if e.classname == "info_player_start" {
            this.spawns.push(e.origin);
        }
    }
    if this.spawns.len() == 0 { throw "no spawns"; }
}
fn on_player_joined(g, id) { g.spawn_at(id, this.spawns[0], 90.0); }
fn tick(g) {
    this.ticks += 1;
    if this.ticks == 5 { g.emit("five", #{ now: g.now(), roll: g.rand(1000) }); }
}
fn on_command(g, id, name, args) {
    if name == "unlock" { g.set_movement_locked(false); }
    if name == "arm" { g.give_weapon(id, "eb", 5); }
    name == "ping" || name == "unlock" || name == "arm"
}
fn on_damage(g, victim, attacker, amount) { g.apply_damage(victim, amount); }
fn hud(g, id) { #{ health: g.health(id), ticks: this.ticks } }
"#;

fn spawn_entity() -> Entity {
    Entity {
        properties: vec![
            ("classname".into(), "info_player_start".into()),
            ("origin".into(), "10 20 24".into()),
        ],
    }
}

fn host(seed: u64) -> GameHost {
    GameHost::new(SCRIPT, Arc::new(Floor), &[spawn_entity()], &[], None, seed).unwrap()
}

#[test]
fn script_discovers_entities_and_spawns_players() {
    let mut host = host(1);
    let id = host.add_player(Team::Alpha).unwrap();
    host.start_match().unwrap();
    let events = host.take_events();
    let spawn = events.iter().find(|e| e.kind == "spawn").unwrap();
    assert_eq!(spawn.vec3("origin"), Some([10.0, 20.0, 24.0]));
    assert_eq!(host.player(id).origin, [10.0, 20.0, 24.0]);
}

#[test]
fn ticks_advance_the_clock_by_one_tick_each() {
    let mut host = host(1);
    host.add_player(Team::Alpha).unwrap();
    for _ in 0..5 {
        host.tick().unwrap();
    }
    assert_eq!(host.now_ms(), 5 * qf_engine::game::TICK_MS);
    let five = host
        .take_events()
        .into_iter()
        .find(|e| e.kind == "five")
        .unwrap();
    assert_eq!(five.int("now"), Some(40));
    assert_eq!(host.hud(0).unwrap().get("ticks"), Some(&Value::Int(5)));
}

#[test]
fn same_seed_gives_same_random_numbers_and_different_seeds_differ() {
    let roll = |seed| {
        let mut host = host(seed);
        host.add_player(Team::Alpha).unwrap();
        for _ in 0..5 {
            host.tick().unwrap();
        }
        host.take_events()
            .into_iter()
            .find(|e| e.kind == "five")
            .and_then(|e| e.int("roll"))
            .unwrap()
    };
    assert_eq!(roll(7), roll(7));
    assert_ne!(roll(7), roll(8));
}

#[test]
fn fire_input_runs_weapons_and_routes_damage_through_the_script() {
    use qf_engine::game::{PlayerInput, WeaponEvent};
    let mut host = host(1);
    let shooter = host.add_player(Team::Alpha).unwrap();
    let victim = host.add_player(Team::Beta).unwrap();
    host.set_player_motion(victim, [310.0, 20.0, 24.0], [0.0; 3], 0.0, 0.0);
    host.set_player_motion(shooter, [10.0, 20.0, 24.0], [0.0; 3], 0.0, 0.0);
    host.command(shooter, "arm", "").unwrap();
    host.command(shooter, "unlock", "").unwrap();
    let fire = PlayerInput {
        fire: true,
        ..PlayerInput::default()
    };
    for _ in 0..12 {
        host.set_player_input(shooter, fire);
        host.tick().unwrap();
    }
    let events = host.take_weapon_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, WeaponEvent::Damage { victim: v, .. } if *v == victim))
    );
    assert!(host.player(victim).health < 100.0);
    assert!(
        host.player(shooter)
            .inventory
            .ammo(qf_engine::game::WeaponId::Electrobolt)
            < 5
    );
    assert!(host.take_impulse(victim)[0] > 0.0);
}

#[test]
fn weapons_stay_silent_while_movement_is_locked() {
    use qf_engine::game::PlayerInput;
    let mut host = host(1);
    let shooter = host.add_player(Team::Alpha).unwrap();
    host.command(shooter, "arm", "").unwrap();
    for _ in 0..12 {
        host.set_player_input(
            shooter,
            PlayerInput {
                fire: true,
                ..PlayerInput::default()
            },
        );
        host.tick().unwrap();
    }
    assert!(host.take_weapon_events().is_empty());
}

#[test]
fn commands_damage_and_errors_go_through_the_script() {
    let mut host = host(1);
    let id = host.add_player(Team::Alpha).unwrap();
    assert!(host.command(id, "ping", "").unwrap());
    assert!(!host.command(id, "pong", "").unwrap());
    host.damage(id, None, 150.0).unwrap();
    let player = host.player(id);
    assert!(player.health < 100.0);
    assert!(player.armor < 100.0);
}

#[test]
fn script_errors_surface_as_results() {
    let error = GameHost::new(SCRIPT, Arc::new(Floor), &[], &[], None, 1)
        .expect_err("init throws without spawns");
    assert!(error.to_string().contains("no spawns"), "{error}");
    let missing = GameHost::new("fn tick(g) {}", Arc::new(Floor), &[], &[], None, 1);
    assert!(missing.is_err(), "init is a required hook");
}
