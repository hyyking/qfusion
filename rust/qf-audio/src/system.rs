//! Game-facing audio front end: turns engine [`SoundEvent`]s into mixer commands.

use std::collections::{HashMap, HashSet};

use qf_engine::sound::{
    PlayerSound, SOUND_JUMPPAD, SOUND_LAUNCHPAD, SOUND_TELEPORT_IN, SOUND_TELEPORT_OUT,
    SOUND_WALLJUMP_FAILED, SoundChannel, SoundEvent, SoundName, SoundPosition,
};

use crate::cache::{SoundCache, SoundSource};
use crate::mixer::{Command, MixerHandle, Placement, PlayParams, Priority, VoiceId};
use crate::spatial::Listener;

/// Default of the legacy `s_volume` cvar.
pub const DEFAULT_MASTER_VOLUME: f32 = 0.8;
/// Model used for `*sound` names until the client selects another one.
pub const DEFAULT_PLAYER_MODEL: &str = "padpork";
/// Entity id used for channel replacement of the local player's sounds.
const LOCAL_PLAYER_ENTITY: u32 = u32::MAX;

fn channel_number(channel: SoundChannel) -> u8 {
    match channel {
        SoundChannel::Auto => 0,
        SoundChannel::Pain => 1,
        SoundChannel::Voice => 2,
        SoundChannel::Item => 3,
        SoundChannel::Body => 4,
        SoundChannel::MuzzleFlash => 5,
    }
}

/// Number of recorded variants for each player sound in the stock content.
fn variants(sound: PlayerSound) -> u32 {
    match sound {
        PlayerSound::Jump | PlayerSound::Dash | PlayerSound::WallJump => 2,
        PlayerSound::Fall(_) => 1,
    }
}

/// Resolves a model-dependent sound (`*jump_1`) to an asset path; `variant` wraps around.
pub fn player_sound_path(model: &str, sound: PlayerSound, variant: u32) -> String {
    let n = variant % variants(sound) + 1;
    let file = match sound {
        PlayerSound::Jump => format!("jump_{n}"),
        PlayerSound::Dash => format!("dash_{n}"),
        PlayerSound::WallJump => format!("wj_{n}"),
        PlayerSound::Fall(level) => format!("fall_{}", level.min(2)),
    };
    format!("sounds/players/{model}/{file}")
}

/// Every asset the built-in events can request, for preloading and diagnostics.
pub fn builtin_sound_paths(model: &str) -> Vec<String> {
    let mut paths = vec![
        SOUND_TELEPORT_IN.to_string(),
        SOUND_TELEPORT_OUT.to_string(),
        SOUND_JUMPPAD.to_string(),
        SOUND_LAUNCHPAD.to_string(),
        SOUND_WALLJUMP_FAILED.to_string(),
    ];
    for sound in [PlayerSound::Jump, PlayerSound::Dash, PlayerSound::WallJump] {
        for v in 0..variants(sound) {
            paths.push(player_sound_path(model, sound, v));
        }
    }
    for level in 0..3 {
        paths.push(player_sound_path(model, PlayerSound::Fall(level), 0));
    }
    paths
}

pub struct AudioSystem<S> {
    handle: MixerHandle,
    cache: SoundCache<S>,
    player_model: String,
    loops: HashMap<u32, VoiceId>,
    reported: HashSet<String>,
    reports: Vec<String>,
    rng: u32,
}

impl<S: SoundSource> AudioSystem<S> {
    pub fn new(mut handle: MixerHandle, source: S) -> Self {
        handle.send(Command::SetMasterVolume(DEFAULT_MASTER_VOLUME));
        Self {
            handle,
            cache: SoundCache::new(source),
            player_model: DEFAULT_PLAYER_MODEL.to_string(),
            loops: HashMap::new(),
            reported: HashSet::new(),
            reports: Vec::new(),
            rng: 0x2545_f491,
        }
    }

    pub fn set_player_model(&mut self, model: &str) {
        self.player_model = model.to_string();
    }

    pub fn set_master_volume(&mut self, volume: f32) {
        self.handle.send(Command::SetMasterVolume(volume));
    }

    pub fn set_listener(&mut self, origin: [f32; 3], yaw_degrees: f32) {
        self.handle.send(Command::SetListener(Listener::from_yaw(
            origin,
            yaw_degrees,
        )));
    }

    pub fn stop_all(&mut self) {
        self.loops.clear();
        self.handle.send(Command::StopAll);
    }

    pub fn dropped_commands(&self) -> u64 {
        self.handle.dropped_commands()
    }

    /// One message per distinct failure (missing or undecodable sound); the caller shows
    /// them. Nothing is ever substituted for a sound that failed to load.
    pub fn take_reports(&mut self) -> Vec<String> {
        std::mem::take(&mut self.reports)
    }

    fn report(&mut self, key: &str, message: String) {
        if self.reported.insert(key.to_string()) {
            self.reports.push(message);
        }
    }

    /// Loads sounds ahead of time so gameplay never stalls on disk or decoding.
    pub fn preload<I, N>(&mut self, names: I)
    where
        I: IntoIterator<Item = N>,
        N: AsRef<str>,
    {
        for name in names {
            let name = name.as_ref();
            if let Err(error) = self.cache.get(name) {
                self.report(name, error.to_string());
            }
        }
    }

    pub fn preload_builtin(&mut self) {
        let paths = builtin_sound_paths(&self.player_model);
        self.preload(paths);
    }

    fn next_random(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    fn resolve(&mut self, name: &SoundName) -> String {
        match name {
            SoundName::Path(path) => path.clone(),
            SoundName::Player(sound) => {
                let variant = self.next_random() >> 8;
                player_sound_path(&self.player_model, *sound, variant)
            }
        }
    }

    pub fn handle_event(&mut self, event: &SoundEvent) {
        match event {
            SoundEvent::Play {
                sound,
                position,
                volume,
                channel,
            } => {
                let id = self.handle.next_voice_id();
                let channel = channel_number(*channel);
                self.start(sound, *position, *volume, false, id, channel);
            }
            SoundEvent::LoopStart {
                key,
                sound,
                position,
                volume,
            } => {
                if let Some(old) = self.loops.remove(key) {
                    self.handle.send(Command::Stop(old));
                }
                let id = self.handle.next_voice_id();
                if self.start(sound, *position, *volume, true, id, 0) {
                    self.loops.insert(*key, id);
                }
            }
            SoundEvent::LoopStop { key } => {
                if let Some(id) = self.loops.remove(key) {
                    self.handle.send(Command::Stop(id));
                }
            }
        }
    }

    fn start(
        &mut self,
        sound: &SoundName,
        position: SoundPosition,
        volume: f32,
        looping: bool,
        id: VoiceId,
        channel: u8,
    ) -> bool {
        let path = self.resolve(sound);
        let buffer = match self.cache.get(&path) {
            Ok(buffer) => buffer,
            Err(error) => {
                self.report(&path, error.to_string());
                return false;
            }
        };
        let (placement, priority) = match position {
            SoundPosition::World {
                origin,
                attenuation,
            } if attenuation > 0.0 => (
                Placement::World {
                    origin,
                    rolloff: attenuation,
                },
                if looping {
                    Priority::Loop
                } else {
                    Priority::OneShot
                },
            ),
            _ => (
                Placement::Local,
                if looping {
                    Priority::Loop
                } else {
                    Priority::Local
                },
            ),
        };
        let channel_key = (channel != 0 && matches!(placement, Placement::Local))
            .then_some((LOCAL_PLAYER_ENTITY, channel));
        self.handle.send(Command::Play {
            buffer,
            params: PlayParams {
                id,
                priority,
                placement,
                volume: volume.clamp(0.0, 1.0),
                pitch: 1.0,
                looping,
                channel_key,
            },
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::tests::make_wav;
    use crate::mixer::{self, Mixer};
    use qf_engine::sound::{ATTN_NORM, SoundChannel};

    const RATE: u32 = 8000;

    #[derive(Default)]
    struct Files(HashMap<String, Vec<u8>>);

    impl SoundSource for Files {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.get(path).cloned()
        }
    }

    fn system(files: &[(&str, Vec<u8>)]) -> (AudioSystem<Files>, Mixer) {
        let (handle, mixer) = mixer::create(RATE);
        let mut src = Files::default();
        for (name, data) in files {
            src.0.insert(name.to_string(), data.clone());
        }
        (AudioSystem::new(handle, src), mixer)
    }

    fn tone() -> Vec<u8> {
        make_wav(RATE, 1, &[16384; 800])
    }

    fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        mixer.render(&mut out);
        out
    }

    fn play_world(sys: &mut AudioSystem<Files>, name: &str, origin: [f32; 3]) {
        sys.handle_event(&SoundEvent::Play {
            sound: SoundName::Path(name.into()),
            position: SoundPosition::World {
                origin,
                attenuation: ATTN_NORM,
            },
            volume: 1.0,
            channel: SoundChannel::Auto,
        });
    }

    #[test]
    fn player_sound_paths_follow_legacy_names() {
        assert_eq!(
            player_sound_path("padpork", PlayerSound::Jump, 0),
            "sounds/players/padpork/jump_1"
        );
        assert_eq!(
            player_sound_path("padpork", PlayerSound::WallJump, 3),
            "sounds/players/padpork/wj_2"
        );
        assert_eq!(
            player_sound_path("x", PlayerSound::Fall(2), 9),
            "sounds/players/x/fall_2"
        );
        assert!(builtin_sound_paths("padpork").contains(&"sounds/world/tele_in".to_string()));
    }

    #[test]
    fn spatial_event_is_audible_panned_and_attenuated() {
        let (mut sys, mut mixer) = system(&[("sounds/a.wav", tone())]);
        sys.set_listener([0.0; 3], 0.0);
        sys.set_master_volume(1.0);
        play_world(&mut sys, "sounds/a", [0.0, -250.0, 0.0]);
        let out = render(&mut mixer, 64);
        assert!(
            out[0].abs() < 1e-5,
            "left ear silent for a source on the right"
        );
        assert!(
            (out[1] - 0.5).abs() < 1e-4,
            "within reference distance: full gain"
        );
        assert!(sys.take_reports().is_empty());
    }

    #[test]
    fn missing_sound_is_reported_once_and_not_played() {
        let (mut sys, mut mixer) = system(&[]);
        for _ in 0..3 {
            play_world(&mut sys, "sounds/nope", [0.0; 3]);
        }
        let reports = sys.take_reports();
        assert_eq!(reports.len(), 1);
        assert!(reports[0].contains("sounds/nope"));
        assert!(render(&mut mixer, 32).iter().all(|&s| s == 0.0));
        assert_eq!(mixer.active_voices(), 0);
    }

    #[test]
    fn looping_speaker_plays_until_stopped() {
        let (mut sys, mut mixer) = system(&[("sounds/loop.wav", make_wav(RATE, 1, &[16384; 100]))]);
        sys.set_master_volume(1.0);
        sys.handle_event(&SoundEvent::LoopStart {
            key: 3,
            sound: SoundName::Path("sounds/loop".into()),
            position: SoundPosition::Local,
            volume: 1.0,
        });
        let out = render(&mut mixer, 1000);
        assert!(
            out.iter().all(|&s| (s - 0.5).abs() < 1e-5),
            "loops past its length"
        );
        // Restarting the same key does not stack voices.
        sys.handle_event(&SoundEvent::LoopStart {
            key: 3,
            sound: SoundName::Path("sounds/loop".into()),
            position: SoundPosition::Local,
            volume: 1.0,
        });
        render(&mut mixer, 256);
        assert_eq!(mixer.active_voices(), 1);
        sys.handle_event(&SoundEvent::LoopStop { key: 3 });
        render(&mut mixer, 256);
        assert_eq!(mixer.active_voices(), 0);
    }

    #[test]
    fn player_event_picks_an_existing_variant() {
        let (mut sys, mut mixer) = system(&[
            ("sounds/players/padpork/jump_1.wav", tone()),
            ("sounds/players/padpork/jump_2.wav", tone()),
        ]);
        for _ in 0..8 {
            sys.handle_event(&SoundEvent::Play {
                sound: SoundName::Player(PlayerSound::Jump),
                position: SoundPosition::Local,
                volume: 1.0,
                channel: SoundChannel::Body,
            });
        }
        assert!(sys.take_reports().is_empty());
        render(&mut mixer, 16);
        assert_eq!(
            mixer.active_voices(),
            1,
            "same channel replaces the previous jump"
        );
    }

    #[test]
    fn preload_reports_every_missing_builtin_once() {
        let (mut sys, _mixer) = system(&[]);
        sys.preload_builtin();
        sys.preload_builtin();
        assert_eq!(
            sys.take_reports().len(),
            builtin_sound_paths("padpork").len()
        );
    }
}
