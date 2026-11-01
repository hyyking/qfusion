//! Headless end-to-end render: engine events -> AudioSystem -> mixer -> PCM, no device.
//! Uses the Warsow install for real ogg/wav content and skips when it is absent
//! (override the location with `QF_BASEWSW`).

use std::path::PathBuf;

use qf_audio::AudioSystem;
use qf_audio::mixer;
use qf_engine::content::bsp::entities::parse_entities;
use qf_engine::content::vfs::Vfs;
use qf_engine::sound::{
    ATTN_IDLE, PlayerSound, SoundChannel, SoundEvent, SoundName, SoundPosition, SpeakerSet,
};

const RATE: u32 = 48_000;

fn game_vfs() -> Option<Vfs> {
    let dir = PathBuf::from(r"C:\Program Files (x86)\Warsow 2.1\basewsw");
    let dir = std::env::var_os("QF_BASEWSW").map_or(dir, PathBuf::from);
    if !dir.is_dir() {
        eprintln!("skipping: {} not found", dir.display());
        return None;
    }
    let mut vfs = Vfs::new();
    vfs.mount_pk3_dir(&dir).ok()?;
    Some(vfs)
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn real_content_renders_and_pans() {
    let Some(vfs) = game_vfs() else { return };
    let (handle, mut mixer) = mixer::create(RATE);
    let mut audio = AudioSystem::new(handle, vfs);
    audio.preload_builtin();
    let reports = audio.take_reports();
    // tele_out ships in no stock pk3; it must be reported, never substituted.
    assert!(
        reports.len() == 1 && reports[0].contains("tele_out"),
        "unexpected reports: {reports:#?}"
    );

    audio.set_listener([0.0; 3], 0.0);
    audio.handle_event(&SoundEvent::Play {
        sound: SoundName::Player(PlayerSound::Dash),
        position: SoundPosition::Local,
        volume: 1.0,
        channel: SoundChannel::Body,
    });
    let mut out = vec![0.0; RATE as usize / 10 * 2];
    mixer.render(&mut out);
    assert!(peak(&out) > 0.01, "decoded ogg is audible");
    mixer.render(&mut vec![0.0; RATE as usize * 4]); // let the dash finish
    assert_eq!(mixer.active_voices(), 0);

    audio.handle_event(&SoundEvent::Play {
        sound: SoundName::Path("sounds/world/tele_in".into()),
        position: SoundPosition::World {
            origin: [0.0, -400.0, 0.0],
            attenuation: ATTN_IDLE,
        },
        volume: 1.0,
        channel: SoundChannel::Auto,
    });
    let mut out = vec![0.0; RATE as usize / 4 * 2];
    mixer.render(&mut out);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert!(peak(&right) > 0.001);
    assert!(
        peak(&left) < 1e-6,
        "hard-right source is silent on the left"
    );
}

#[test]
fn speaker_names_resolve_and_ambient_loop_runs() {
    let Some(vfs) = game_vfs() else { return };
    let speakers = SpeakerSet::from_entities(
        &parse_entities(
            r#"{ "classname" "target_speaker" "origin" "0 0 0" "noise" "sounds/ambient/ceilingfan.ogg"
                "attenuation" "3" "spawnflags" "1" }
               { "classname" "target_speaker" "origin" "0 0 0" "noise" "sounds/world/dong.wav" "spawnflags" "16" }"#,
        )
        .unwrap(),
    );
    let (handle, mut mixer) = mixer::create(RATE);
    let mut audio = AudioSystem::new(handle, vfs);
    audio.preload(speakers.speakers.iter().map(|s| s.noise.clone()));
    assert!(audio.take_reports().is_empty());
    for event in speakers.initial_events() {
        audio.handle_event(&event);
    }
    audio.set_listener([100.0, 0.0, 0.0], 0.0);
    let mut out = vec![0.0; RATE as usize * 2];
    for _ in 0..30 {
        mixer.render(&mut out);
    }
    assert_eq!(mixer.active_voices(), 1, "loop outlives the sample length");
    assert!(peak(&out) > 0.0001);
}

#[test]
fn missing_content_is_reported() {
    let Some(vfs) = game_vfs() else { return };
    let (handle, _mixer) = mixer::create(RATE);
    let mut audio = AudioSystem::new(handle, vfs);
    audio.preload(["sounds/definitely/missing"]);
    let reports = audio.take_reports();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].contains("not found"));
}
