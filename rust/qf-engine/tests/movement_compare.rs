//! Movement recording replay and legacy comparison (see `docs/movement-recording.md`).
//!
//! The legacy side is the checked-in `wbomb1_sample.legacy.qfrec`, produced by the
//! `pmove_replay` tool from the same recording. Tests that need the real wbomb1 pack skip
//! when it is not installed (`WARSOW_BASEWSW` overrides the directory).

use std::path::PathBuf;

use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use qf_engine::simulation::bsp_world::BspWorld;
use qf_engine::simulation::compare::{Tolerances, compare, first_exact_mismatch};
use qf_engine::simulation::pmove::PmoveRules;
use qf_engine::simulation::recording::{Recording, TickResult, replay};

const SAMPLE: &str = include_str!("fixtures/wbomb1_sample.qfrec");
const LEGACY: &str = include_str!("fixtures/wbomb1_sample.legacy.qfrec");

fn results(recording: &Recording) -> Vec<TickResult> {
    recording.ticks.iter().map(|t| t.result.clone()).collect()
}

fn wbomb1() -> Option<Vec<u8>> {
    let base = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = PathBuf::from(base).join("map_wbomb1.pk3");
    if !pk3.exists() {
        eprintln!("skipping: {} unavailable", pk3.display());
        return None;
    }
    Some(read_pk3_entry(&pk3, "maps/wbomb1.bsp").expect("wbomb1.bsp in pack"))
}

#[test]
fn fixtures_round_trip_through_text() {
    let sample = Recording::from_text(SAMPLE).expect("sample parses");
    assert_eq!(sample.to_text(), SAMPLE.replace("\r\n", "\n"));
    let legacy = Recording::from_text(LEGACY).expect("legacy parses");
    assert_eq!(sample.ticks.len(), legacy.ticks.len());
    for (a, b) in sample.ticks.iter().zip(&legacy.ticks) {
        assert_eq!(
            a.command, b.command,
            "legacy replay must use the same inputs"
        );
    }
}

#[test]
fn legacy_output_matches_recorded_rust_state_within_tolerances() {
    let sample = Recording::from_text(SAMPLE).unwrap();
    let legacy = Recording::from_text(LEGACY).unwrap();
    let comparison = compare(&results(&sample), &results(&legacy), &Tolerances::default());
    assert!(
        comparison.passed(),
        "first divergence: {:?}",
        comparison.first_divergence()
    );
}

#[test]
fn rust_replay_on_real_wbomb1_is_bit_exact_and_matches_legacy() {
    let Some(data) = wbomb1() else { return };
    let sample = Recording::from_text(SAMPLE).unwrap();
    assert!(
        sample.map.matches(&data),
        "fixture was recorded on another wbomb1 build"
    );
    let bsp = Bsp::parse(&data).unwrap();
    let world = BspWorld::new(&bsp);
    let replayed = replay(&world, &PmoveRules::default(), &sample);
    assert_eq!(first_exact_mismatch(&replayed, &results(&sample)), None);

    let legacy = Recording::from_text(LEGACY).unwrap();
    let comparison = compare(&replayed, &results(&legacy), &Tolerances::default());
    assert!(comparison.passed(), "{:?}", comparison.first_divergence());
}

#[test]
fn a_perturbed_recording_is_reported_at_the_first_divergent_tick() {
    let sample = Recording::from_text(SAMPLE).unwrap();
    let mut legacy = results(&Recording::from_text(LEGACY).unwrap());
    legacy[300].origin[0] += 1.0;
    let comparison = compare(&results(&sample), &legacy, &Tolerances::default());
    assert!(!comparison.passed());
    assert_eq!(comparison.first_divergence().map(|d| d.tick), Some(300));
}
