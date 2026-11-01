//! Real-time mixer: voices, priorities, looping, resampling and spatial gains.
//!
//! [`Mixer`] is owned by the audio callback. Everything it needs arrives through a
//! lock-free single-producer ring buffer of [`Command`]s sent by the [`MixerHandle`]; the
//! callback itself never locks, allocates or frees memory.
//!
//! Buffers are shared as `Arc<Buffer>`. The sound cache keeps a reference for as long as
//! the buffer may be playing, so a voice finishing inside the callback never drops the
//! last reference.

use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::spatial::{Listener, spatial_gains};

/// Same voice limit as the legacy `MAX_SRC`.
pub const MAX_VOICES: usize = 128;
const COMMAND_CAPACITY: usize = 512;

/// Immutable decoded sound shared between the cache and playing voices.
#[derive(Debug)]
pub struct Buffer {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Box<[f32]>,
}

impl Buffer {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_secs(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct VoiceId(pub u32);

/// Legacy source priorities (`SRCPRI_*`): a new sound may only displace a voice of lower
/// priority, or an older one of equal priority.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Priority {
    Ambient = 0,
    Loop = 1,
    OneShot = 2,
    Local = 3,
    Stream = 4,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// Centered at full gain.
    Local,
    World {
        origin: [f32; 3],
        rolloff: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayParams {
    pub id: VoiceId,
    pub priority: Priority,
    pub placement: Placement,
    pub volume: f32,
    pub pitch: f32,
    pub looping: bool,
    /// `(entity, channel)`; a new voice replaces the one with the same key.
    pub channel_key: Option<(u32, u8)>,
}

pub enum Command {
    Play {
        buffer: Arc<Buffer>,
        params: PlayParams,
    },
    Stop(VoiceId),
    StopAll,
    SetListener(Listener),
    SetMasterVolume(f32),
}

/// Control-side endpoint. Cheap to call from the game thread; never blocks.
pub struct MixerHandle {
    producer: Producer<Command>,
    next_id: u32,
    dropped_commands: u64,
}

impl MixerHandle {
    pub fn next_voice_id(&mut self) -> VoiceId {
        self.next_id = self.next_id.wrapping_add(1);
        VoiceId(self.next_id)
    }

    /// Returns `false` (and counts the loss) when the queue is full.
    pub fn send(&mut self, command: Command) -> bool {
        let ok = self.producer.push(command).is_ok();
        if !ok {
            self.dropped_commands += 1;
        }
        ok
    }

    pub fn dropped_commands(&self) -> u64 {
        self.dropped_commands
    }
}

struct Voice {
    buffer: Arc<Buffer>,
    params: PlayParams,
    /// Position in source frames.
    position: f64,
    step: f64,
    gain_left: f32,
    gain_right: f32,
    serial: u64,
    stopping: bool,
}

pub struct Mixer {
    sample_rate: u32,
    consumer: Consumer<Command>,
    voices: Vec<Option<Voice>>,
    listener: Listener,
    master: f32,
    serial: u64,
    dropped_voices: u64,
}

/// Creates the control handle and the mixer that renders what the handle queues.
pub fn create(sample_rate: u32) -> (MixerHandle, Mixer) {
    let (producer, consumer) = RingBuffer::new(COMMAND_CAPACITY);
    let mut voices = Vec::with_capacity(MAX_VOICES);
    voices.resize_with(MAX_VOICES, || None);
    (
        MixerHandle {
            producer,
            next_id: 0,
            dropped_commands: 0,
        },
        Mixer {
            sample_rate,
            consumer,
            voices,
            listener: Listener::default(),
            master: 1.0,
            serial: 0,
            dropped_voices: 0,
        },
    )
}

fn target_gains(voice: &Voice, listener: &Listener, master: f32) -> (f32, f32) {
    if voice.stopping {
        return (0.0, 0.0);
    }
    let volume = voice.params.volume * master;
    let (l, r) = match voice.params.placement {
        Placement::Local => (1.0, 1.0),
        Placement::World { rolloff, .. } if rolloff <= 0.0 => (1.0, 1.0),
        Placement::World { origin, rolloff } => spatial_gains(listener, origin, rolloff),
    };
    (l * volume, r * volume)
}

impl Mixer {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().flatten().count()
    }

    /// Voices that could not be started because everything playing outranked them.
    pub fn dropped_voices(&self) -> u64 {
        self.dropped_voices
    }

    fn drain_commands(&mut self) {
        while let Ok(command) = self.consumer.pop() {
            match command {
                Command::Play { buffer, params } => self.start(buffer, params),
                Command::Stop(id) => {
                    for voice in self.voices.iter_mut().flatten() {
                        if voice.params.id == id {
                            voice.stopping = true;
                        }
                    }
                }
                Command::StopAll => {
                    for voice in self.voices.iter_mut().flatten() {
                        voice.stopping = true;
                    }
                }
                Command::SetListener(listener) => self.listener = listener,
                Command::SetMasterVolume(volume) => self.master = volume.clamp(0.0, 4.0),
            }
        }
    }

    /// Mirrors `SourceManager::allocSource`.
    fn pick_slot(&self, params: &PlayParams) -> Option<usize> {
        if let Some((entity, channel)) = params.channel_key
            && channel != 0
            && let Some(slot) = self.voices.iter().position(|v| {
                v.as_ref()
                    .is_some_and(|v| v.params.channel_key == Some((entity, channel)))
            })
        {
            return Some(slot);
        }
        if let Some(slot) = self.voices.iter().position(Option::is_none) {
            return Some(slot);
        }
        let mut worst: Option<(usize, Priority, u64)> = None;
        let mut worst_priority = params.priority;
        let mut worst_serial = self.serial;
        for (slot, voice) in self.voices.iter().enumerate() {
            let Some(voice) = voice else { continue };
            let p = voice.params.priority;
            if p < worst_priority || (p == worst_priority && voice.serial < worst_serial) {
                worst = Some((slot, p, voice.serial));
                worst_priority = p;
                worst_serial = voice.serial;
            }
        }
        worst.map(|(slot, _, _)| slot)
    }

    fn start(&mut self, buffer: Arc<Buffer>, params: PlayParams) {
        if buffer.frames() == 0 || !(params.pitch.is_finite() && params.pitch > 0.0) {
            return;
        }
        let Some(slot) = self.pick_slot(&params) else {
            self.dropped_voices += 1;
            return;
        };
        self.serial += 1;
        let step =
            f64::from(buffer.sample_rate) / f64::from(self.sample_rate) * f64::from(params.pitch);
        let mut voice = Voice {
            buffer,
            params,
            position: 0.0,
            step,
            gain_left: 0.0,
            gain_right: 0.0,
            serial: self.serial,
            stopping: false,
        };
        // Start at the target gain so the transient of a one-shot is not softened.
        (voice.gain_left, voice.gain_right) = target_gains(&voice, &self.listener, self.master);
        self.voices[slot] = Some(voice);
    }

    /// Renders `out.len() / 2` stereo frames (interleaved) into `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        self.drain_commands();
        out.fill(0.0);
        let frames = out.len() / 2;
        if frames == 0 {
            return;
        }
        let inv_frames = 1.0 / frames as f32;
        for slot in self.voices.iter_mut() {
            let Some(voice) = slot else { continue };
            let (target_l, target_r) = target_gains(voice, &self.listener, self.master);
            let step_l = (target_l - voice.gain_left) * inv_frames;
            let step_r = (target_r - voice.gain_right) * inv_frames;
            let (mut gl, mut gr) = (voice.gain_left, voice.gain_right);
            let buffer = &*voice.buffer;
            let total = buffer.frames();
            let channels = usize::from(buffer.channels);
            let spatial = !matches!(voice.params.placement, Placement::Local);
            let mut finished = false;

            for frame in out.chunks_exact_mut(2) {
                if voice.position >= total as f64 {
                    if voice.params.looping {
                        voice.position %= total as f64;
                    } else {
                        finished = true;
                        break;
                    }
                }
                let i0 = voice.position as usize;
                let frac = (voice.position - i0 as f64) as f32;
                let i1 = if i0 + 1 < total {
                    i0 + 1
                } else if voice.params.looping {
                    0
                } else {
                    i0
                };
                let at = |i: usize, c: usize| buffer.samples[i * channels + c];
                let (left, right) = if channels == 1 {
                    let s = at(i0, 0) + (at(i1, 0) - at(i0, 0)) * frac;
                    (s, s)
                } else {
                    let l = at(i0, 0) + (at(i1, 0) - at(i0, 0)) * frac;
                    let r = at(i0, 1) + (at(i1, 1) - at(i0, 1)) * frac;
                    if spatial {
                        let m = 0.5 * (l + r);
                        (m, m)
                    } else {
                        (l, r)
                    }
                };
                gl += step_l;
                gr += step_r;
                frame[0] += left * gl;
                frame[1] += right * gr;
                voice.position += voice.step;
            }
            voice.gain_left = target_l;
            voice.gain_right = target_r;
            if finished || voice.stopping {
                *slot = None;
            }
        }
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn buffer(rate: u32, channels: u16, samples: Vec<f32>) -> Arc<Buffer> {
        Arc::new(Buffer {
            name: "test".into(),
            sample_rate: rate,
            channels,
            samples: samples.into(),
        })
    }

    fn params(id: u32, priority: Priority) -> PlayParams {
        PlayParams {
            id: VoiceId(id),
            priority,
            placement: Placement::Local,
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            channel_key: None,
        }
    }

    fn play(h: &mut MixerHandle, b: &Arc<Buffer>, p: PlayParams) {
        assert!(h.send(Command::Play {
            buffer: b.clone(),
            params: p
        }));
    }

    fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        m.render(&mut out);
        out
    }

    #[test]
    fn silence_without_voices() {
        let (_h, mut m) = create(RATE);
        assert!(render(&mut m, 256).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn one_shot_plays_then_frees_its_voice() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.5; 100]);
        play(&mut h, &b, params(1, Priority::OneShot));
        let out = render(&mut m, 64);
        assert_eq!((out[0], out[1]), (0.5, 0.5));
        assert_eq!(m.active_voices(), 1);
        let out = render(&mut m, 64);
        assert!(out[2 * 36 - 2] > 0.4 && out[2 * 40] == 0.0);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn looping_voice_wraps_until_stopped() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.25; 10]);
        let mut p = params(7, Priority::Loop);
        p.looping = true;
        play(&mut h, &b, p);
        let out = render(&mut m, 1000);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6));
        assert_eq!(m.active_voices(), 1);
        h.send(Command::Stop(VoiceId(7)));
        let tail = render(&mut m, 128);
        assert_eq!(m.active_voices(), 0);
        assert!(tail[tail.len() - 1].abs() < 1e-6, "fade reaches silence");
        assert!(render(&mut m, 16).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn resamples_to_device_rate() {
        let (mut h, mut m) = create(RATE);
        // A 24 kHz ramp played at 48 kHz interpolates between source frames.
        let b = buffer(24_000, 1, vec![0.0, 1.0, 0.0, 1.0]);
        play(&mut h, &b, params(1, Priority::OneShot));
        let out = render(&mut m, 8);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left[0], 0.0);
        assert_eq!(left[1], 0.5);
        assert_eq!(left[2], 1.0);
        assert_eq!(left[3], 0.5);
    }

    #[test]
    fn pitch_changes_playback_speed() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        let mut p = params(1, Priority::OneShot);
        p.pitch = 2.0;
        play(&mut h, &b, p);
        let left: Vec<f32> = render(&mut m, 3).iter().step_by(2).copied().collect();
        assert!((left[1] - 0.4).abs() < 1e-6 && (left[2] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn world_voice_pans_and_attenuates() {
        let (mut h, mut m) = create(RATE);
        h.send(Command::SetListener(Listener::from_yaw([0.0; 3], 0.0)));
        let b = buffer(RATE, 1, vec![1.0; 4000]);
        let mut p = params(1, Priority::OneShot);
        // Hard right (negative y when facing +x) and three times the reference distance.
        p.placement = Placement::World {
            origin: [0.0, -750.0, 0.0],
            rolloff: 1.0,
        };
        play(&mut h, &b, p);
        let out = render(&mut m, 32);
        assert!(out[0].abs() < 1e-6);
        assert!((out[1] - 1.0 / 3.0).abs() < 1e-5);
    }

    #[test]
    fn zero_rolloff_world_voice_is_global() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![1.0; 100]);
        let mut p = params(1, Priority::OneShot);
        p.placement = Placement::World {
            origin: [1.0e5, 0.0, 0.0],
            rolloff: 0.0,
        };
        play(&mut h, &b, p);
        let out = render(&mut m, 4);
        assert_eq!((out[0], out[1]), (1.0, 1.0));
    }

    #[test]
    fn stereo_buffer_keeps_channels_when_local_and_downmixes_when_spatial() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 2, vec![1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
        play(&mut h, &b, params(1, Priority::Local));
        let out = render(&mut m, 2);
        assert_eq!((out[0], out[1]), (1.0, 0.0));
    }

    #[test]
    fn master_volume_and_voice_volume_scale_output() {
        let (mut h, mut m) = create(RATE);
        h.send(Command::SetMasterVolume(0.5));
        let b = buffer(RATE, 1, vec![1.0; 100]);
        let mut p = params(1, Priority::OneShot);
        p.volume = 0.5;
        play(&mut h, &b, p);
        assert_eq!(render(&mut m, 1)[0], 0.25);
    }

    #[test]
    fn output_is_clamped() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.9; 100]);
        for i in 0..4 {
            play(&mut h, &b, params(i, Priority::OneShot));
        }
        assert!(render(&mut m, 8).iter().all(|&s| s == 1.0));
    }

    #[test]
    fn same_entity_channel_replaces_previous_voice() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.1; 1000]);
        for id in 0..3 {
            let mut p = params(id, Priority::OneShot);
            p.channel_key = Some((5, 2));
            play(&mut h, &b, p);
        }
        render(&mut m, 4);
        assert_eq!(m.active_voices(), 1);
        // Channel 0 ("auto") never overrides.
        for id in 10..13 {
            let mut p = params(id, Priority::OneShot);
            p.channel_key = Some((5, 0));
            play(&mut h, &b, p);
        }
        render(&mut m, 4);
        assert_eq!(m.active_voices(), 4);
    }

    #[test]
    fn full_mixer_steals_lowest_priority_then_oldest() {
        let (mut h, mut m) = create(RATE);
        let b = buffer(RATE, 1, vec![0.001; 100_000]);
        let mut ambient = params(0, Priority::Ambient);
        ambient.looping = true;
        play(&mut h, &b, ambient);
        for id in 1..MAX_VOICES as u32 {
            play(&mut h, &b, params(id, Priority::OneShot));
            if id % 64 == 0 {
                render(&mut m, 1);
            }
        }
        render(&mut m, 1);
        assert_eq!(m.active_voices(), MAX_VOICES);
        // A one-shot displaces the ambient loop, the lowest priority voice.
        play(&mut h, &b, params(500, Priority::OneShot));
        render(&mut m, 1);
        let ids: Vec<u32> = m.voices.iter().flatten().map(|v| v.params.id.0).collect();
        assert!(!ids.contains(&0) && ids.contains(&500));
        // An ambient sound cannot displace anything of higher priority.
        play(&mut h, &b, params(501, Priority::Ambient));
        render(&mut m, 1);
        assert_eq!(m.dropped_voices(), 1);
        // An equal-priority one-shot replaces the oldest one-shot (id 1).
        play(&mut h, &b, params(502, Priority::OneShot));
        render(&mut m, 1);
        let ids: Vec<u32> = m.voices.iter().flatten().map(|v| v.params.id.0).collect();
        assert!(!ids.contains(&1) && ids.contains(&502));
    }

    #[test]
    fn command_queue_reports_overflow_instead_of_blocking() {
        let (mut h, _m) = create(RATE);
        let mut accepted = 0;
        for _ in 0..(COMMAND_CAPACITY + 10) {
            if h.send(Command::StopAll) {
                accepted += 1;
            }
        }
        assert_eq!(accepted, COMMAND_CAPACITY);
        assert_eq!(h.dropped_commands(), 10);
    }

    #[test]
    fn empty_or_invalid_voices_are_ignored() {
        let (mut h, mut m) = create(RATE);
        play(
            &mut h,
            &buffer(RATE, 1, vec![]),
            params(1, Priority::OneShot),
        );
        let mut p = params(2, Priority::OneShot);
        p.pitch = f32::NAN;
        play(&mut h, &buffer(RATE, 1, vec![1.0; 4]), p);
        render(&mut m, 4);
        assert_eq!(m.active_voices(), 0);
    }
}
