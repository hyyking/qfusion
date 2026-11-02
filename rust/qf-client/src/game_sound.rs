//! Client side of game audio: mounts game content, owns the output device and plays the
//! device-independent sound events emitted by qf-engine.

use std::path::{Path, PathBuf};

use qf_audio::{AudioDevice, AudioSystem};
use qf_engine::content::bsp::Bsp;
use qf_engine::content::vfs::Vfs;
use qf_engine::simulation::entities::{MoverPose, TriggerEvent};
use qf_engine::simulation::pmove::PredictedEvent;
use qf_engine::sound::{
    ATTN_NORM, MoverSounds, SOUND_TELEPORT_IN, SOUND_TELEPORT_OUT, SoundChannel, SoundEvent,
    SoundName, SoundPosition, SpeakerSet, sound_for_pmove_event, sounds_for_trigger_event,
};

const EXPLOSION_SOUND: &str = "sounds/weapons/rocket_strong_explosion";

const TEST_SOUND: &str = "sounds/menu/ok";

/// Stock install locations of the Warsow base game, searched when no `--game-dir` is given.
fn default_game_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(r"C:\Program Files (x86)\Warsow 2.1\basewsw")];
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(Path::new(&appdata).join("Warsow 2.1").join("basewsw"));
    }
    dirs
}

pub fn mount_content(game_dirs: &[PathBuf], map_pk3: Option<&Path>) -> Vfs {
    let mut vfs = Vfs::new();
    let defaults;
    let dirs = if game_dirs.is_empty() {
        defaults = default_game_dirs();
        &defaults[..]
    } else {
        game_dirs
    };
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        match vfs.mount_pk3_dir(dir) {
            Ok(n) => println!("content: mounted {n} packs from {}", dir.display()),
            Err(e) => eprintln!("content: cannot mount {}: {e}", dir.display()),
        }
    }
    if let Some(pk3) = map_pk3
        && let Err(e) = vfs.mount_pk3(pk3)
    {
        eprintln!("content: cannot mount {}: {e}", pk3.display());
    }
    vfs
}

pub struct GameSound {
    // Keeps the output stream alive; dropping it stops playback.
    _device: AudioDevice,
    system: AudioSystem<Vfs>,
    speakers: SpeakerSet,
    movers: MoverSounds,
    last_eye: Option<[f32; 3]>,
}

impl GameSound {
    /// Starts the device and loads the map's `target_speaker` entities.
    pub fn new(vfs: Vfs, bsp: &Bsp) -> Result<Self, qf_audio::AudioError> {
        let (device, handle) = AudioDevice::start_default()?;
        let mut system = AudioSystem::new(handle, vfs);
        // The stock packs ship no tele_out; reuse tele_in (reported once).
        system.add_fallback(SOUND_TELEPORT_OUT, SOUND_TELEPORT_IN);
        system.preload_builtin();
        system.preload([TEST_SOUND, EXPLOSION_SOUND]);
        let speakers = SpeakerSet::from_entities(&bsp.parsed_entities);
        for warning in &speakers.warnings {
            eprintln!(
                "target_speaker (entity {}): {}",
                warning.entity, warning.message
            );
        }
        system.preload(speakers.speakers.iter().map(|s| s.noise.as_str()));
        for event in speakers.initial_events() {
            system.handle_event(&event);
        }
        let bounds: Vec<_> = bsp.models.iter().map(|m| (m.mins, m.maxs)).collect();
        let movers = MoverSounds::from_entities(&bsp.parsed_entities, &bounds);
        for mover in movers.sounds() {
            system.preload(
                [&mover.start, &mover.stop, &mover.moving]
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        let mut sound = Self {
            _device: device,
            system,
            speakers,
            movers,
            last_eye: None,
        };
        sound.print_reports();
        Ok(sound)
    }

    /// Call once per frame with the view origin and yaw (degrees) and the frame's trigger events.
    pub fn update(
        &mut self,
        eye: [f32; 3],
        yaw: f32,
        movement: Vec<PredictedEvent>,
        triggers: &[TriggerEvent],
        movers: &[MoverPose],
        dt: f32,
    ) {
        for event in self.movers.update(movers, dt) {
            self.system.handle_event(&event);
        }
        let from = self.last_eye.replace(eye).unwrap_or(eye);
        self.system.set_listener(eye, yaw);
        for event in movement {
            self.system.handle_event(&sound_for_pmove_event(event));
        }
        for trigger in triggers {
            for event in sounds_for_trigger_event(trigger, from, eye) {
                self.system.handle_event(&event);
            }
        }
        self.print_reports();
    }

    /// Plays the strong rocket explosion at each origin (`EV_EXPLOSION2` in the legacy client).
    pub fn play_explosions(&mut self, origins: &[[f32; 3]]) {
        for &origin in origins {
            self.system.handle_event(&SoundEvent::Play {
                sound: SoundName::Path(EXPLOSION_SOUND.into()),
                position: SoundPosition::World {
                    origin,
                    attenuation: ATTN_NORM,
                },
                volume: 1.0,
                channel: SoundChannel::Auto,
            });
        }
        self.print_reports();
    }

    pub fn play_test_sound(&mut self) {
        self.system.handle_event(&SoundEvent::Play {
            sound: SoundName::Path(TEST_SOUND.into()),
            position: SoundPosition::Local,
            volume: 1.0,
            channel: SoundChannel::Auto,
        });
        self.print_reports();
    }

    /// Fires `target_speaker` entities with this targetname (for a future trigger system).
    #[allow(dead_code)]
    pub fn trigger(&mut self, targetname: &str, activator_origin: [f32; 3]) {
        for event in self.speakers.trigger(targetname, activator_origin) {
            self.system.handle_event(&event);
        }
    }

    fn print_reports(&mut self) {
        for report in self.system.take_reports() {
            eprintln!("content: {report}");
        }
    }
}
