//! Scripted Bomb and Defuse rounds on the real wbomb1 map (skipped when it is missing).

mod common;

use std::sync::Arc;

use common::Harness;
use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use qf_engine::game::{BOMB_SCRIPT, GameHost, Team};
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::pmove::{
    CONTENTS_SOLID, PLAYER_STAND_MAXS, PLAYER_STAND_MINS, PmoveWorld, Vec3,
};

const ALPHA: usize = 0;
const BETA: usize = 1;

fn load() -> Option<Arc<Bsp>> {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = std::path::Path::new(&base).join("map_wbomb1.pk3");
    match read_pk3_entry(&pk3, "maps/wbomb1.bsp") {
        Ok(data) => Some(Arc::new(Bsp::parse(&data).unwrap())),
        Err(_) => {
            eprintln!("skipping: {} unavailable", pk3.display());
            None
        }
    }
}

fn harness_with(bsp: &Arc<Bsp>, script: &str, seed: u64) -> Harness {
    let mut host = GameHost::from_bsp(script, bsp.clone(), seed).unwrap();
    assert_eq!(host.add_player(Team::Alpha).unwrap(), ALPHA);
    assert_eq!(host.add_player(Team::Beta).unwrap(), BETA);
    host.start_match().unwrap();
    Harness::new(host, 2)
}

fn harness(bsp: &Arc<Bsp>, seed: u64) -> Harness {
    harness_with(bsp, BOMB_SCRIPT, seed)
}

/// A standable position inside the first capture-area trigger (site A).
fn plant_position(bsp: &Bsp) -> Vec3 {
    let trigger = bsp
        .parsed_entities
        .iter()
        .find(|e| e.get("classname") == Some("trigger_capture_area"))
        .unwrap();
    let model = &bsp.models[trigger.get("model").unwrap()[1..].parse::<usize>().unwrap()];
    let center = [
        (model.mins[0] + model.maxs[0]) / 2.0,
        (model.mins[1] + model.maxs[1]) / 2.0,
        model.maxs[2],
    ];
    let trace = BspWorld::new(bsp).trace(
        center,
        PLAYER_STAND_MINS,
        PLAYER_STAND_MAXS,
        [center[0], center[1], model.mins[2] - 64.0],
        CONTENTS_SOLID,
    );
    assert!(!trace.start_solid, "trigger center is solid");
    trace.end_position
}

/// The Alpha attacker plants at site A and holds position until the bomb is armed.
fn plant_and_arm(h: &mut Harness, bsp: &Bsp) -> Vec3 {
    h.start_round();
    let position = plant_position(bsp);
    h.held[ALPHA] = Some(position);
    h.held[BETA] = Some(h.spawn_of(BETA));
    h.run_until(2_000, |h| h.bomb_state() == "planting");
    h.run_until(6_000, |h| h.bomb_state() == "armed");
    position
}

#[test]
fn script_discovers_wbomb1_bomb_entities() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 1);
    h.run_until(20_000, |h| h.round_state() == "round");
    let given = h.find("bomb_given");
    assert_eq!(given.len(), 1);
    assert_eq!(given[0].int("player"), Some(ALPHA as i64));
}

#[test]
fn plant_and_explosion_wins_for_attackers() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    plant_and_arm(&mut h, &bsp);
    h.run_until(40_000, |h| h.bomb_state() == "exploded");
    assert_eq!(h.round_won(), Some((0, "bomb_exploded".into())));
    assert_eq!(h.hud_text(0, "round_state"), "finished");
    h.run_until(5_000, |h| h.round_state() == "post");
    assert_eq!(
        h.find("round_won").len(),
        1,
        "blast kills must not score twice"
    );
    assert_eq!(h.find("bomb_exploded").len(), 1);
}

#[test]
fn defuse_wins_for_defenders() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    let position = plant_and_arm(&mut h, &bsp);
    // The attacker leaves; the defender walks up to the bomb and defuses.
    h.held[ALPHA] = Some(h.spawn_of(ALPHA));
    h.held[BETA] = Some([position[0] + 24.0, position[1], position[2]]);
    h.run_until(10_000, |h| h.bomb_state() == "defused");
    assert_eq!(h.round_won(), Some((1, "bomb_defused".into())));
    assert_eq!(h.host.player(BETA).stats.get("defuses"), Some(&1));
}

#[test]
fn timeout_without_plant_wins_for_defenders() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    h.start_round();
    h.held = vec![Some(h.spawn_of(ALPHA)), Some(h.spawn_of(BETA))];
    h.run_until(61_000, |h| h.round_state() == "finished");
    assert_eq!(h.round_won(), Some((1, "timeout".into())));
}

#[test]
fn elimination_of_defenders_wins_for_attackers() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    h.start_round();
    h.held = vec![Some(h.spawn_of(ALPHA)), Some(h.spawn_of(BETA))];
    h.tick();
    h.host.damage(BETA, Some(ALPHA), 1_000.0).unwrap();
    h.tick();
    assert_eq!(h.round_won(), Some((0, "elimination".into())));
    assert!(!h.host.player(BETA).alive);
    assert_eq!(h.host.player(ALPHA).stats.get("kills"), Some(&1));
}

#[test]
fn elimination_of_attackers_wins_for_defenders_and_drops_bomb() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    h.start_round();
    h.held = vec![Some(h.spawn_of(ALPHA)), Some(h.spawn_of(BETA))];
    h.tick();
    h.host.damage(ALPHA, Some(BETA), 1_000.0).unwrap();
    h.tick();
    assert_eq!(h.round_won(), Some((1, "elimination".into())));
    assert_eq!(h.find("bomb_dropped").len(), 1);
}

#[test]
fn next_round_keeps_roles_early_and_respawns() {
    let Some(bsp) = load() else { return };
    let mut h = harness(&bsp, 7);
    h.start_round();
    h.held = vec![Some(h.spawn_of(ALPHA)), Some(h.spawn_of(BETA))];
    h.host.damage(BETA, None, 1_000.0).unwrap();
    h.run_until(10_000, |h| h.find("round_starting").len() == 2);
    assert!(h.host.player(BETA).alive);
    assert_eq!(h.round_state(), "pre");
    assert!(h.host.movement_locked());
    assert_eq!(h.find("round_starting")[1].int("attacker"), Some(0));
}

#[test]
fn match_ends_at_score_limit_with_two_round_lead() {
    let Some(bsp) = load() else { return };
    let script = BOMB_SCRIPT
        .replace("score_limit: 16", "score_limit: 2")
        .replace("round_count >= limit * 2", "false")
        .replace("round_count == limit", "false");
    let mut h = harness_with(&bsp, &script, 3);
    h.held = vec![None, None];
    for _ in 0..200_000 {
        if !h.find("match_won").is_empty() {
            break;
        }
        if h.round_state() == "round" {
            h.host.damage(BETA, None, 1_000.0).unwrap();
        }
        h.tick();
    }
    assert_eq!(h.find("match_won")[0].int("team"), Some(0));
    assert_eq!(h.hud_text(0, "match_state"), "postmatch");
}

fn fingerprint(bsp: &Arc<Bsp>, seed: u64) -> Vec<qf_engine::game::GameEvent> {
    let mut h = harness(bsp, seed);
    plant_and_arm(&mut h, bsp);
    h.run_until(40_000, |h| h.bomb_state() == "exploded");
    h.run_until(5_000, |h| h.round_state() == "post");
    h.events
}

#[test]
fn replays_are_deterministic() {
    let Some(bsp) = load() else { return };
    assert_eq!(fingerprint(&bsp, 11), fingerprint(&bsp, 11));
}
