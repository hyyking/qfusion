//! Device-independent sound requests emitted by the simulation.
//!
//! The engine never touches an audio device: it produces [`SoundEvent`]s and the
//! client decides how (and whether) to play them. Attenuation values are the legacy
//! OpenAL rolloff factors from `gs_public.h` (`S_DEFAULT_ATTENUATION_MODEL 3`).

use crate::content::bsp::entities::Entity;
use crate::simulation::entities::TriggerEvent;
use crate::simulation::pmove::{PmEvent, PredictedEvent};

pub const ATTN_NONE: f32 = 0.0;
pub const ATTN_DISTANT: f32 = 0.5;
pub const ATTN_NORM: f32 = 1.0;
pub const ATTN_IDLE: f32 = 2.5;
pub const ATTN_STATIC: f32 = 5.0;
pub const ATTN_WHISPER: f32 = 10.0;

/// Legacy entity channels. A non-`Auto` channel replaces the sound already playing on
/// the same entity and channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SoundChannel {
    Auto,
    Pain,
    Voice,
    Item,
    Body,
    MuzzleFlash,
}

/// Sounds that depend on the player model (`*jump_1` and friends in legacy content).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PlayerSound {
    Jump,
    Dash,
    WallJump,
    /// Landing grunt; 0 is a light landing, 2 the heaviest.
    Fall(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SoundName {
    /// Asset path without a mandatory extension, e.g. `sounds/world/tele_in`.
    Path(String),
    Player(PlayerSound),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundPosition {
    /// Heard at full volume, centered, wherever the listener is.
    Local,
    /// Positioned in the world with the legacy distance rolloff factor. An
    /// attenuation of zero behaves like `Local`.
    World { origin: [f32; 3], attenuation: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SoundEvent {
    Play {
        sound: SoundName,
        position: SoundPosition,
        volume: f32,
        channel: SoundChannel,
    },
    /// Starts (or restarts) a looping sound identified by `key`.
    LoopStart {
        key: u32,
        sound: SoundName,
        position: SoundPosition,
        volume: f32,
    },
    LoopStop {
        key: u32,
    },
}

pub const SOUND_TELEPORT_IN: &str = "sounds/world/tele_in";
pub const SOUND_TELEPORT_OUT: &str = "sounds/world/tele_out";
pub const SOUND_JUMPPAD: &str = "sounds/world/jumppad";
pub const SOUND_LAUNCHPAD: &str = "sounds/world/launchpad";
pub const SOUND_WALLJUMP_FAILED: &str = "sounds/world/ft_walljump_failed";

fn local(sound: SoundName, channel: SoundChannel) -> SoundEvent {
    SoundEvent::Play {
        sound,
        position: SoundPosition::Local,
        volume: 1.0,
        channel,
    }
}

/// Maps a movement event of the local player to its sound, following `cg_main.cpp`.
pub fn sound_for_pmove_event(event: PredictedEvent) -> SoundEvent {
    match event.event {
        PmEvent::Jump | PmEvent::DoubleJump | PmEvent::JumpPad => {
            local(SoundName::Player(PlayerSound::Jump), SoundChannel::Body)
        }
        PmEvent::Dash => local(SoundName::Player(PlayerSound::Dash), SoundChannel::Body),
        PmEvent::WallJump => local(SoundName::Player(PlayerSound::WallJump), SoundChannel::Body),
        PmEvent::WallJumpFailed => local(
            SoundName::Path(SOUND_WALLJUMP_FAILED.into()),
            SoundChannel::Body,
        ),
        PmEvent::Fall => {
            let severity = match event.parameter {
                d if d > 10 => 2,
                d if d > 0 => 1,
                _ => 0,
            };
            local(
                SoundName::Player(PlayerSound::Fall(severity)),
                SoundChannel::Pain,
            )
        }
    }
}

/// Sounds for gameplay triggers. The trigger system (jump pads, teleporters) calls
/// these when it fires so the client only ever consumes [`SoundEvent`]s.
pub fn jump_pad_sounds(origin: [f32; 3]) -> [SoundEvent; 2] {
    [
        local(SoundName::Player(PlayerSound::Jump), SoundChannel::Body),
        SoundEvent::Play {
            sound: SoundName::Path(SOUND_JUMPPAD.into()),
            position: SoundPosition::World {
                origin,
                attenuation: ATTN_NORM,
            },
            volume: 1.0,
            channel: SoundChannel::Auto,
        },
    ]
}

pub fn teleport_sound(origin: [f32; 3], entering: bool) -> SoundEvent {
    SoundEvent::Play {
        sound: SoundName::Path(
            if entering {
                SOUND_TELEPORT_IN
            } else {
                SOUND_TELEPORT_OUT
            }
            .into(),
        ),
        position: SoundPosition::World {
            origin,
            attenuation: ATTN_NORM,
        },
        volume: 1.0,
        channel: SoundChannel::Auto,
    }
}

/// Sounds for a trigger event. `from` is the player's position before the frame ran
/// and `to` the position after it (they differ for teleporters).
pub fn sounds_for_trigger_event(
    event: &TriggerEvent,
    from: [f32; 3],
    to: [f32; 3],
) -> Vec<SoundEvent> {
    match event {
        // The player's jump voice comes from the pmove JumpPad event.
        TriggerEvent::JumpPad { .. } => {
            let [_, pad] = jump_pad_sounds(from);
            vec![pad]
        }
        TriggerEvent::Teleport { .. } => {
            vec![teleport_sound(from, false), teleport_sound(to, true)]
        }
        _ => Vec::new(),
    }
}

pub const SPEAKER_LOOPED_ON: u32 = 1;
pub const SPEAKER_LOOPED_OFF: u32 = 2;
pub const SPEAKER_GLOBAL: u32 = 4;
pub const SPEAKER_RELATIVE: u32 = 8;
pub const SPEAKER_ACTIVATOR: u32 = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct Speaker {
    /// Index into the map entity list; stable key for loop state and triggering.
    pub entity: usize,
    pub targetname: Option<String>,
    pub origin: [f32; 3],
    pub noise: String,
    pub volume: f32,
    pub attenuation: f32,
    pub spawnflags: u32,
    pub looping_on: bool,
}

impl Speaker {
    pub fn is_looping(&self) -> bool {
        self.spawnflags & (SPEAKER_LOOPED_ON | SPEAKER_LOOPED_OFF) != 0
    }

    fn position(&self) -> SoundPosition {
        if self.attenuation == ATTN_NONE {
            SoundPosition::Local
        } else {
            SoundPosition::World {
                origin: self.origin,
                attenuation: self.attenuation,
            }
        }
    }

    fn loop_start(&self) -> SoundEvent {
        SoundEvent::LoopStart {
            key: self.entity as u32,
            sound: SoundName::Path(self.noise.clone()),
            position: self.position(),
            volume: self.volume,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeakerWarning {
    pub entity: usize,
    pub message: String,
}

/// `target_speaker` entities of a map with their runtime loop state.
#[derive(Clone, Debug, Default)]
pub struct SpeakerSet {
    pub speakers: Vec<Speaker>,
    pub warnings: Vec<SpeakerWarning>,
}

fn parse_origin(value: &str) -> Option<[f32; 3]> {
    let mut parts = value.split_whitespace().map(|p| p.parse::<f32>());
    let origin = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    (parts.next().is_none() && origin.iter().all(|v| v.is_finite())).then_some(origin)
}

impl SpeakerSet {
    /// Mirrors `SP_target_speaker`: entities without a noise are skipped, attenuation `-1`
    /// or the GLOBAL flag disables distance rolloff, and an unset attenuation means
    /// `ATTN_NORM`.
    pub fn from_entities(entities: &[Entity]) -> Self {
        let mut set = Self::default();
        for (index, entity) in entities.iter().enumerate() {
            if !entity
                .get("classname")
                .is_some_and(|c| c.eq_ignore_ascii_case("target_speaker"))
            {
                continue;
            }
            let noise = entity.get("noise").map(str::trim).unwrap_or_default();
            if noise.is_empty() {
                set.warnings.push(SpeakerWarning {
                    entity: index,
                    message: "target_speaker with no noise".into(),
                });
                continue;
            }
            let Some(origin) = entity.get("origin").and_then(parse_origin) else {
                set.warnings.push(SpeakerWarning {
                    entity: index,
                    message: format!("target_speaker {noise} has no valid origin"),
                });
                continue;
            };
            let spawnflags = entity
                .get("spawnflags")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .unwrap_or(0);
            let attenuation = match entity.get("attenuation").map(|v| v.trim().parse::<f32>()) {
                _ if spawnflags & SPEAKER_GLOBAL != 0 => ATTN_NONE,
                Some(Ok(-1.0)) => ATTN_NONE,
                Some(Ok(a)) if a.is_finite() && a > 0.0 => a,
                _ => ATTN_NORM,
            };
            let volume = match entity.get("volume").map(|v| v.trim().parse::<f32>()) {
                Some(Ok(v)) if v.is_finite() && v > 0.0 => v.min(1.0),
                _ => 1.0,
            };
            set.speakers.push(Speaker {
                entity: index,
                targetname: entity.get("targetname").map(str::to_string),
                origin,
                noise: noise.to_string(),
                volume,
                attenuation,
                spawnflags,
                looping_on: spawnflags & SPEAKER_LOOPED_ON != 0,
            });
        }
        set
    }

    /// Events needed at map start: every pre-started loop.
    pub fn initial_events(&self) -> Vec<SoundEvent> {
        self.speakers
            .iter()
            .filter(|s| s.looping_on)
            .map(Speaker::loop_start)
            .collect()
    }

    /// Activates every speaker with this targetname. Looping speakers toggle; others
    /// play once. `activator_origin` is used for the RELATIVE flag.
    pub fn trigger(&mut self, targetname: &str, activator_origin: [f32; 3]) -> Vec<SoundEvent> {
        let mut events = Vec::new();
        for speaker in &mut self.speakers {
            if speaker.targetname.as_deref() != Some(targetname) {
                continue;
            }
            if speaker.is_looping() {
                speaker.looping_on = !speaker.looping_on;
                events.push(if speaker.looping_on {
                    speaker.loop_start()
                } else {
                    SoundEvent::LoopStop {
                        key: speaker.entity as u32,
                    }
                });
                continue;
            }
            let position = if speaker.spawnflags & SPEAKER_RELATIVE != 0 {
                SoundPosition::World {
                    origin: activator_origin,
                    attenuation: speaker.attenuation,
                }
            } else if speaker.spawnflags & SPEAKER_ACTIVATOR != 0 {
                SoundPosition::Local
            } else {
                speaker.position()
            };
            events.push(SoundEvent::Play {
                sound: SoundName::Path(speaker.noise.clone()),
                position,
                volume: speaker.volume,
                channel: if speaker.spawnflags & SPEAKER_RELATIVE != 0 {
                    SoundChannel::Voice
                } else {
                    SoundChannel::Auto
                },
            });
        }
        events
    }
}

pub const SOUND_DOOR_START: &str = "sounds/movers/door_start";
pub const SOUND_DOOR_STOP: &str = "sounds/movers/door_stop";
pub const SOUND_PLAT_MOVE: &str = "sounds/movers/elevator_move";

/// Loop keys of movers are tagged so they never collide with speaker keys.
const MOVER_LOOP_TAG: u32 = 0x8000_0000;
/// A mover that has not changed pose for this long counts as stopped.
const MOVER_IDLE_SECONDS: f32 = 0.1;

/// One mover's sounds; `None` means silent.
#[derive(Clone, Debug, PartialEq)]
pub struct MoverSound {
    pub entity: usize,
    pub model: usize,
    /// Center of the brush model at rest, relative to the pose origin.
    pub center: [f32; 3],
    pub start: Option<String>,
    pub stop: Option<String>,
    pub moving: Option<String>,
}

/// Door, plat and rotating-door sounds. Derived on the client from pose changes, so the
/// simulation stays free of audio concerns.
#[derive(Debug, Default)]
pub struct MoverSounds {
    movers: Vec<MoverSound>,
    state: Vec<MoverState>,
}

#[derive(Debug, Default, Clone)]
struct MoverState {
    last: Option<([f32; 3], [f32; 3])>,
    moving: bool,
    idle: f32,
}

fn mover_noise(entity: &Entity, key: &str, default: Option<&str>) -> Option<String> {
    match entity.get(key) {
        Some("silent") => None,
        Some(v) if v != "default" && !v.is_empty() => Some(v.to_string()),
        _ => default.map(str::to_string),
    }
}

impl MoverSounds {
    /// `bounds` are the `(mins, maxs)` of each BSP submodel.
    pub fn from_entities(entities: &[Entity], bounds: &[([f32; 3], [f32; 3])]) -> Self {
        let mut movers = Vec::new();
        for (index, entity) in entities.iter().enumerate() {
            let defaults = match entity
                .get("classname")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "func_door" | "func_door_rotating" => {
                    (Some(SOUND_DOOR_START), Some(SOUND_DOOR_STOP), None)
                }
                "func_plat" => (None, None, Some(SOUND_PLAT_MOVE)),
                _ => continue,
            };
            let Some(model) = entity
                .get("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|m| m.parse::<usize>().ok())
                .filter(|&m| m != 0)
            else {
                continue;
            };
            let center = bounds.get(model).map_or([0.0; 3], |(mins, maxs)| {
                std::array::from_fn(|i| (mins[i] + maxs[i]) * 0.5)
            });
            let sound = MoverSound {
                entity: index,
                model,
                center,
                start: mover_noise(entity, "noise_start", defaults.0),
                stop: mover_noise(entity, "noise_stop", defaults.1),
                moving: mover_noise(entity, "noise", defaults.2),
            };
            if sound.start.is_some() || sound.stop.is_some() || sound.moving.is_some() {
                movers.push(sound);
            }
        }
        let state = vec![MoverState::default(); movers.len()];
        Self { movers, state }
    }

    pub fn sounds(&self) -> &[MoverSound] {
        &self.movers
    }

    /// Diffs this frame's poses against the previous ones. `dt` is the frame time in seconds.
    pub fn update(
        &mut self,
        poses: &[crate::simulation::entities::MoverPose],
        dt: f32,
    ) -> Vec<SoundEvent> {
        let mut events = Vec::new();
        for (mover, state) in self.movers.iter().zip(self.state.iter_mut()) {
            let Some(pose) = poses.iter().find(|p| p.entity == mover.entity) else {
                continue;
            };
            let now = (pose.origin, pose.angles);
            let changed = state.last.is_some_and(|last| last != now);
            state.last = Some(now);
            let position = SoundPosition::World {
                origin: std::array::from_fn(|i| pose.origin[i] + mover.center[i]),
                attenuation: ATTN_STATIC,
            };
            let play = |name: &Option<String>| {
                name.as_ref().map(|n| SoundEvent::Play {
                    sound: SoundName::Path(n.clone()),
                    position,
                    volume: 1.0,
                    channel: SoundChannel::Auto,
                })
            };
            let key = mover.entity as u32 | MOVER_LOOP_TAG;
            if changed {
                state.idle = 0.0;
                if !state.moving {
                    state.moving = true;
                    events.extend(play(&mover.start));
                    if let Some(name) = &mover.moving {
                        events.push(SoundEvent::LoopStart {
                            key,
                            sound: SoundName::Path(name.clone()),
                            position,
                            volume: 1.0,
                        });
                    }
                }
            } else if state.moving {
                state.idle += dt;
                if state.idle >= MOVER_IDLE_SECONDS {
                    state.moving = false;
                    if mover.moving.is_some() {
                        events.push(SoundEvent::LoopStop { key });
                    }
                    events.extend(play(&mover.stop));
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::bsp::entities::parse_entities;
    use crate::simulation::entities::MoverPose;

    fn pose(entity: usize, z: f32) -> MoverPose {
        MoverPose {
            entity,
            model: 1,
            origin: [0.0, 0.0, z],
            angles: [0.0; 3],
        }
    }

    #[test]
    fn door_plays_start_then_stop_and_plat_loops() {
        let entities = parse_entities(
            r#"{ "classname" "worldspawn" }
            { "classname" "func_door" "model" "*1" }
            { "classname" "func_plat" "model" "*2" }
            { "classname" "func_door" "model" "*3" "noise_start" "silent" }
            { "classname" "func_button" "model" "*4" }"#,
        )
        .unwrap();
        let bounds = vec![([0.0; 3], [0.0; 3]); 5];
        let mut m = MoverSounds::from_entities(&entities, &bounds);
        assert_eq!(m.sounds().len(), 3);
        assert!(
            m.update(&[pose(1, 0.0), pose(2, 0.0), pose(3, 0.0)], 0.01)
                .is_empty()
        );
        let ev = m.update(&[pose(1, 1.0), pose(2, 1.0), pose(3, 1.0)], 0.01);
        let plays = ev
            .iter()
            .filter(|e| matches!(e, SoundEvent::Play { .. }))
            .count();
        let loops = ev
            .iter()
            .filter(|e| matches!(e, SoundEvent::LoopStart { .. }))
            .count();
        assert_eq!((plays, loops), (1, 1));
        let still = [pose(1, 1.0), pose(2, 1.0), pose(3, 1.0)];
        assert!(m.update(&still, 0.05).is_empty());
        let ev = m.update(&still, 0.06);
        let plays = ev
            .iter()
            .filter(|e| matches!(e, SoundEvent::Play { .. }))
            .count();
        let stops = ev
            .iter()
            .filter(|e| matches!(e, SoundEvent::LoopStop { .. }))
            .count();
        assert_eq!((plays, stops), (2, 1));
    }

    fn speakers(text: &str) -> SpeakerSet {
        SpeakerSet::from_entities(&parse_entities(text).unwrap())
    }

    #[test]
    fn parses_looping_ambient_speaker() {
        let set = speakers(
            r#"{ "classname" "worldspawn" }
            { "classname" "target_speaker" "origin" "10 20 30" "noise" "sounds/ambient/fan.ogg"
              "attenuation" "3" "spawnflags" "1" }"#,
        );
        assert_eq!(set.speakers.len(), 1);
        let s = &set.speakers[0];
        assert_eq!(s.entity, 1);
        assert_eq!(s.origin, [10.0, 20.0, 30.0]);
        assert_eq!(s.attenuation, 3.0);
        assert!(s.is_looping() && s.looping_on);
        assert_eq!(set.initial_events().len(), 1);
    }

    #[test]
    fn attenuation_defaults_follow_legacy() {
        let set = speakers(
            r#"{ "classname" "target_speaker" "origin" "0 0 0" "noise" "a" }
            { "classname" "target_speaker" "origin" "0 0 0" "noise" "b" "attenuation" "-1" }
            { "classname" "target_speaker" "origin" "0 0 0" "noise" "c" "attenuation" "3" "spawnflags" "4" }"#,
        );
        let att: Vec<f32> = set.speakers.iter().map(|s| s.attenuation).collect();
        assert_eq!(att, [ATTN_NORM, ATTN_NONE, ATTN_NONE]);
    }

    #[test]
    fn missing_noise_or_origin_is_reported_not_silently_dropped() {
        let set = speakers(
            r#"{ "classname" "target_speaker" "origin" "0 0 0" }
            { "classname" "target_speaker" "noise" "x" }"#,
        );
        assert!(set.speakers.is_empty());
        assert_eq!(set.warnings.len(), 2);
    }

    #[test]
    fn looping_speaker_toggles_and_one_shot_plays() {
        let mut set = speakers(
            r#"{ "classname" "target_speaker" "origin" "0 0 0" "noise" "loop" "targetname" "t" "spawnflags" "2" }
            { "classname" "target_speaker" "origin" "1 0 0" "noise" "shot" "targetname" "t" }"#,
        );
        assert!(set.initial_events().is_empty());
        let first = set.trigger("t", [0.0; 3]);
        assert!(matches!(first[0], SoundEvent::LoopStart { key: 0, .. }));
        assert!(matches!(first[1], SoundEvent::Play { .. }));
        let second = set.trigger("t", [0.0; 3]);
        assert_eq!(second[0], SoundEvent::LoopStop { key: 0 });
        assert!(set.trigger("other", [0.0; 3]).is_empty());
    }

    #[test]
    fn activator_flag_plays_locally() {
        let mut set = speakers(
            r#"{ "classname" "target_speaker" "origin" "9 9 9" "noise" "x" "targetname" "t" "spawnflags" "16" }"#,
        );
        let SoundEvent::Play { position, .. } = &set.trigger("t", [0.0; 3])[0] else {
            panic!("expected play");
        };
        assert_eq!(*position, SoundPosition::Local);
    }

    #[test]
    fn fall_severity_follows_damage() {
        let sev = |parameter| match sound_for_pmove_event(PredictedEvent {
            event: PmEvent::Fall,
            parameter,
        }) {
            SoundEvent::Play {
                sound: SoundName::Player(PlayerSound::Fall(n)),
                ..
            } => n,
            other => panic!("{other:?}"),
        };
        assert_eq!((sev(0), sev(5), sev(11)), (0, 1, 2));
    }

    #[test]
    fn trigger_events_map_to_sounds() {
        let pad = TriggerEvent::JumpPad {
            entity: 0,
            velocity: [0.0; 3],
        };
        assert_eq!(sounds_for_trigger_event(&pad, [1.0; 3], [2.0; 3]).len(), 1);
        let tele = TriggerEvent::Teleport {
            entity: 0,
            destination: crate::simulation::entities::Destination {
                origin: [0.0; 3],
                angles: [0.0; 3],
            },
            velocity: [0.0; 3],
        };
        let sounds = sounds_for_trigger_event(&tele, [1.0; 3], [2.0; 3]);
        assert_eq!(sounds[0], teleport_sound([1.0; 3], false));
        assert_eq!(sounds[1], teleport_sound([2.0; 3], true));
        let kill = TriggerEvent::Kill { entity: 0 };
        assert!(sounds_for_trigger_event(&kill, [0.0; 3], [0.0; 3]).is_empty());
    }

    #[test]
    fn failed_wall_jump_uses_world_sound() {
        let ev = sound_for_pmove_event(PredictedEvent {
            event: PmEvent::WallJumpFailed,
            parameter: 0,
        });
        assert!(matches!(
            ev,
            SoundEvent::Play { sound: SoundName::Path(p), .. } if p == SOUND_WALLJUMP_FAILED
        ));
    }
}
