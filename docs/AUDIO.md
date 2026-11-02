# Game audio architecture

Audio is split so that the simulation stays device-free and testable.

```
qf-engine (no audio types)            qf-audio                               qf-client
  sound::SoundEvent  ───────────────▶  AudioSystem ──▶ SoundCache (Vfs)       game_sound.rs
  sound::SpeakerSet (target_speaker)    │  handle_event / set_listener         mounts packs, owns AudioDevice,
  walk::take_events (pmove events)      ▼                                       feeds events + listener per frame
  content::vfs::Vfs (pk3 + dirs)      MixerHandle ══ rtrb SPSC queue ══▶ Mixer ──▶ cpal callback
```

## qf-engine
- `content::vfs::Vfs`: case-insensitive virtual filesystem over pk3s (alphabetical, later wins) and loose
  directories (override packs).
- `sound`: `SoundEvent` (`Play`, `LoopStart{key}`, `LoopStop{key}`), `SoundName` (path or player sound),
  `SoundPosition` (`Local` or `World{origin, attenuation}`), `SoundChannel`, legacy `ATTN_*` constants.
  `sound_for_pmove_event` maps the pmove events the engine exposes (jump, double jump, dash, walljump, walljump
  failed, fall) to sounds as in `cg_main.cpp`; `jump_pad_sounds` / `teleport_sound` are ready for a future trigger system.
- `SpeakerSet`: `target_speaker` entities (spawnflags LOOPED_ON/LOOPED_OFF/GLOBAL/RELATIVE/ACTIVATOR, `attenuation`,
  `volume`, `noise`); `initial_events()` starts looped-on speakers, `trigger(targetname, activator_origin)` toggles
  loops or plays one-shots. Malformed speakers are reported as warnings, never silently dropped.
- `WalkPlayer::events` / `ModeSimulation::take_events()` expose the pmove events produced by each step.

## qf-audio
- `decode`: WAV (PCM 8/16/24/32, float, mono/stereo) and Ogg Vorbis (`lewton`) to `f32` PCM.
- `cache::SoundCache`: loads through a `SoundSource`, caches successes **and** failures, never substitutes another
  sound. Legacy extension handling: an explicit extension is tried first, then the other (`.wav` <-> `.ogg`); no
  extension tries `.ogg` then `.wav`. Errors are `NotFound` (with the paths tried) or `Decode`.
  The cache owns every buffer for its lifetime, so the audio callback never drops the last `Arc` (no free in callback).
- `spatial`: legacy model: OpenAL inverse-distance-clamped, ref 250, max 8000, `gain = ref / (ref + rolloff *
  (clamp(d, ref, max) - ref))`; equal-power stereo pan from the listener's right vector. `Local`/attenuation 0 is
  centered at full volume.
- `mixer`: 128 voices, priorities (ambient < loop < one-shot < local < stream), legacy stealing (lowest priority,
  then oldest), same entity+channel replaces its voice, looping, pitch, linear resampling to the device rate,
  per-block gain ramps, fade-out on stop, output clamped. The callback does no allocation or locking: voices are
  preallocated, commands arrive over a lock-free SPSC queue (overflow is counted, never blocks).
- `device`: cpal output (f32/i16/u16; mono/stereo/surround front pair).
- `system::AudioSystem`: applies `SoundEvent`s, tracks loops, picks `_1`/`_2` variants, collects deduplicated
  missing/decode reports (`take_reports`).

## Client
`game_sound.rs` mounts `--game-dir` (default: Warsow 2.1 install and `%APPDATA%` `basewsw`) plus the map pk3,
preloads built-in and map sounds, starts the speakers, and each frame sets the listener (eye, yaw) and plays
drained pmove events. `B` plays `sounds/menu/ok`. Missing sounds print once to stderr
(e.g. `sounds/world/tele_out` does not exist in the stock packs; the client registers an opt-in `AudioSystem::add_fallback` of `tele_out` -> `tele_in`, reported once, while the engine default still only reports).

## Explosions
The bomb gametype emits an `explosion` event (`origin`, `radius`) for each blast entity it detonates. The client
(`GameSound::play_explosions`) plays `sounds/weapons/rocket_strong_explosion` there with `ATTN_NORM`, as legacy
`EV_EXPLOSION2` does. The freeplay client has no projectile simulation, so only bomb-site blasts produce it.

## Mover sounds
`MoverSounds` (qf-engine `sound.rs`) covers `func_door`, `func_door_rotating` and `func_plat`. The client derives
start/stop from per-frame `MoverPose` changes (idle for 0.1 s means stopped), so the simulation is untouched. Defaults
follow `G_AssignMoverSounds`: doors play `sounds/movers/door_start` / `door_stop`, plats loop
`sounds/movers/elevator_move`; the `noise_start`, `noise_stop` and `noise` keys override them (`silent` disables,
`default` keeps the default). Sounds are positioned at the brush model center with `ATTN_STATIC`. Known content gap:
`func_button` plays `sounds/movers/button`, which is absent from the stock packs, so buttons stay silent.

## Tests
Unit tests cover decode, attenuation/panning, mixer math and the system. `rust/qf-audio/tests/offline_render.rs`
renders events through the mixer headlessly against real Warsow content (skipped if the install is absent; set
`QF_BASEWSW` to override).

## Not yet ported from the legacy engine
- Footsteps, land, pain/death/drown/gasp sounds (no pmove events/surface types yet); player model selection from
  client info (`padpork` is hardcoded; `*` sexed `target_speaker` noises).
- Trigger sounds: `sounds_for_trigger_event` maps the engine's `TriggerEvent::JumpPad` / `Teleport` to the jump-pad world
  sound and tele_out/tele_in (positions are the player's pre/post-frame view origin); the pad's jump voice comes from
  the pmove `JumpPad` event. Other trigger events are silent.
- OpenAL extras: HRTF, EFX/reverb, occlusion/sound propagation, doppler, per-source cone.
- Music/streaming, per-category volume cvars (`v_volumePlayers`, `v_volumeEffects`), sound entities attached to
  moving entities, `s_*` cvar configuration beyond the master volume.
