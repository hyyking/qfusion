//! Versioned movement recordings: the exact per-tick `usercmd` fed to `pmove`, the initial
//! player state, the map identity and the Rust engine's resulting per-tick state.
//!
//! The text format is documented in `docs/movement-recording.md`. Floats are printed with
//! Rust's shortest round-trip formatting, so parsing a file reproduces the exact `f32`
//! values and replaying it is bit-for-bit deterministic on the same build.

use std::fmt::{self, Write as _};

use super::player::{NoclipPlayer, StepError, TICK_DURATION};
use super::pmove::{
    PmType, PmoveCommand, PmoveResult, PmoveRules, PmoveState, PmoveStats, PmoveTouch, PmoveWorld,
    PredictedEvent, TouchContext, Vec3, pmove, pmove_with_touch,
};
use super::walk::{ModeSimulation, MovementMode, TickLogEntry, WalkCommand};

pub const FORMAT_NAME: &str = "qfusion-movement-recording";
pub const FORMAT_VERSION: u32 = 1;

pub const TICK_COLUMNS: &str = "tick,msec,buttons,forward,side,up,angle0,angle1,angle2,x,y,z,vx,vy,vz,grounded,flags,pm_time,view_height,maxs_z,delta0,delta1,delta2,water_level,ladder,step,trigger,events,touches";

/// Which map a recording was made on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapIdentity {
    pub name: String,
    /// BSP entry or path the map was loaded from.
    pub entry: String,
    pub size: u64,
    /// FNV-1a 64 of the BSP bytes, see [`fnv1a64`].
    pub fnv1a64: u64,
}

impl MapIdentity {
    pub fn of_bsp(name: &str, entry: &str, bytes: &[u8]) -> Self {
        Self {
            name: name.to_string(),
            entry: entry.to_string(),
            size: bytes.len() as u64,
            fnv1a64: fnv1a64(bytes),
        }
    }

    pub fn matches(&self, bytes: &[u8]) -> bool {
        self.size == bytes.len() as u64 && self.fnv1a64 == fnv1a64(bytes)
    }
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The player at the first recorded tick, before any command is applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InitialState {
    pub state: PmoveState,
}

/// Engine output of one tick.
#[derive(Clone, Debug, PartialEq)]
pub struct TickResult {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub grounded: bool,
    pub flags: u32,
    pub pm_time: i32,
    pub view_height: f32,
    pub maxs_z: f32,
    pub delta_angles: [i16; 3],
    pub water_level: u8,
    pub ladder: bool,
    pub step: f32,
    /// Map trigger events (jump pads, teleporters, ...) fired during this tick. Movement-only
    /// replays take the recorded state at these ticks, because the legacy movement
    /// reference has no entity system.
    pub trigger: bool,
    /// `(legacy event id, parameter)` in emission order.
    pub events: Vec<(i32, i32)>,
    pub touches: Vec<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TickRecord {
    pub command: PmoveCommand,
    pub result: TickResult,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recording {
    pub map: MapIdentity,
    pub tick_rate_hz: u32,
    /// Whether map triggers were evaluated (the touch path of pmove, which also refreshes
    /// delta angles after the move). Replays take the same path.
    pub touch_hook: bool,
    pub generator: String,
    pub initial: InitialState,
    pub ticks: Vec<TickRecord>,
}

impl TickResult {
    pub fn from_state(state: &PmoveState, result: &PmoveResult, trigger: bool) -> Self {
        Self {
            origin: state.origin,
            velocity: state.velocity,
            grounded: state.on_ground(),
            flags: state.flags,
            pm_time: state.pm_time,
            view_height: state.view_height,
            maxs_z: state.maxs[2],
            delta_angles: state.delta_angles,
            water_level: result.water_level,
            ladder: result.ladder,
            step: result.step,
            trigger,
            events: result
                .events
                .iter()
                .map(|event| (i32::from(event.event as u8), event.parameter))
                .collect(),
            touches: result.touch_entities.clone(),
        }
    }
}

/// Accumulates logged simulation ticks into a [`Recording`].
pub struct Recorder {
    recording: Recording,
}

impl Recorder {
    pub fn start(
        map: MapIdentity,
        generator: &str,
        tick_rate_hz: u32,
        touch_hook: bool,
        state: &PmoveState,
    ) -> Self {
        Self {
            recording: Recording {
                map,
                tick_rate_hz,
                touch_hook,
                generator: generator.to_string(),
                initial: InitialState { state: *state },
                ticks: Vec::new(),
            },
        }
    }

    pub fn push(&mut self, entry: &TickLogEntry) {
        self.recording.ticks.push(TickRecord {
            command: entry.outcome.command,
            result: TickResult::from_state(
                &entry.state,
                &entry.outcome.result,
                entry.trigger_events > 0,
            ),
        });
    }

    pub fn tick_count(&self) -> usize {
        self.recording.ticks.len()
    }

    pub fn finish(self) -> Recording {
        self.recording
    }
}

/// Trigger hook that touches nothing. Walking in the client always goes through the touch
/// path, which also refreshes delta angles after the move, so replays take it too.
struct NoTouch;

impl PmoveTouch for NoTouch {
    fn touch_triggers(
        &mut self,
        _: &mut PmoveState,
        _: &TouchContext,
        _: &mut Vec<PredictedEvent>,
    ) {
    }
}

/// Replays the commands of a recording through the movement code without map entities,
pub fn replay<W: PmoveWorld>(
    world: &W,
    rules: &PmoveRules,
    recording: &Recording,
) -> Vec<TickResult> {
    replay_with(world, rules, recording, |_| {})
}

///
/// Like [`replay`] but calls the closure with the tick index ahead of each tick.
pub fn replay_with<W: PmoveWorld>(
    world: &W,
    rules: &PmoveRules,
    recording: &Recording,
    mut before_tick: impl FnMut(usize),
) -> Vec<TickResult> {
    let mut state = recording.initial.state;
    let mut out = Vec::with_capacity(recording.ticks.len());
    for (index, tick) in recording.ticks.iter().enumerate() {
        before_tick(index);
        state.pm_type = PmType::Normal;
        let result = if recording.touch_hook {
            pmove_with_touch(world, rules, &mut state, &tick.command, &mut NoTouch)
        } else {
            pmove(world, rules, &mut state, &tick.command)
        };
        if tick.result.trigger {
            apply_recorded(&mut state, &tick.result);
        }
        out.push(TickResult::from_state(&state, &result, tick.result.trigger));
    }
    out
}

/// Overwrites the movement-visible state with a recorded tick result.
pub fn apply_recorded(state: &mut PmoveState, recorded: &TickResult) {
    state.origin = recorded.origin;
    state.velocity = recorded.velocity;
    state.flags = recorded.flags;
    state.pm_time = recorded.pm_time;
    state.view_height = recorded.view_height;
    state.maxs[2] = recorded.maxs_z;
    state.delta_angles = recorded.delta_angles;
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "recording line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

fn join<T: fmt::Display>(values: &[T]) -> String {
    let mut out = String::new();
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{value}");
    }
    out
}

impl Recording {
    pub fn to_text(&self) -> String {
        let s = &self.initial.state;
        let mut out = String::new();
        let _ = writeln!(out, "{FORMAT_NAME} {FORMAT_VERSION}");
        let _ = writeln!(out, "map_name {}", self.map.name);
        let _ = writeln!(out, "map_entry {}", self.map.entry);
        let _ = writeln!(out, "map_size {}", self.map.size);
        let _ = writeln!(out, "map_fnv1a64 {:016x}", self.map.fnv1a64);
        let _ = writeln!(out, "tick_rate_hz {}", self.tick_rate_hz);
        let _ = writeln!(out, "touch_hook {}", u8::from(self.touch_hook));
        let _ = writeln!(out, "generator {}", self.generator);
        let _ = writeln!(out, "start_origin {}", join(&s.origin));
        let _ = writeln!(out, "start_velocity {}", join(&s.velocity));
        let _ = writeln!(out, "start_view_angles {}", join(&s.view_angles));
        let _ = writeln!(out, "start_view_height {}", s.view_height);
        let _ = writeln!(out, "start_mins {}", join(&s.mins));
        let _ = writeln!(out, "start_maxs {}", join(&s.maxs));
        let _ = writeln!(out, "start_flags {}", s.flags);
        let _ = writeln!(out, "start_pm_time {}", s.pm_time);
        let _ = writeln!(out, "start_gravity {}", s.gravity);
        let _ = writeln!(out, "start_delta_angles {}", join(&s.delta_angles));
        for (name, value) in stat_fields(&s.stats) {
            let _ = writeln!(out, "stat_{name} {value}");
        }
        let _ = writeln!(out, "ticks {}", self.ticks.len());
        let _ = writeln!(out, "tick_columns {TICK_COLUMNS}");
        for (index, tick) in self.ticks.iter().enumerate() {
            let c = &tick.command;
            let r = &tick.result;
            let events: Vec<String> = r.events.iter().map(|(id, p)| format!("{id}:{p}")).collect();
            let touches: Vec<String> = r.touches.iter().map(i32::to_string).collect();
            let _ = writeln!(
                out,
                "{index},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                c.msec,
                c.buttons,
                c.forward_move,
                c.side_move,
                c.up_move,
                c.angles[0],
                c.angles[1],
                c.angles[2],
                r.origin[0],
                r.origin[1],
                r.origin[2],
                r.velocity[0],
                r.velocity[1],
                r.velocity[2],
                u8::from(r.grounded),
                r.flags,
                r.pm_time,
                r.view_height,
                r.maxs_z,
                r.delta_angles[0],
                r.delta_angles[1],
                r.delta_angles[2],
                r.water_level,
                u8::from(r.ladder),
                r.step,
                u8::from(r.trigger),
                events.join(";"),
                touches.join(";"),
            );
        }
        out
    }

    pub fn from_text(text: &str) -> Result<Self, ParseError> {
        let mut lines = text.lines().enumerate().map(|(i, l)| (i + 1, l.trim()));
        let err = |line: usize, message: String| ParseError { line, message };
        let (line, magic) = lines
            .find(|(_, l)| !l.is_empty() && !l.starts_with('#'))
            .ok_or_else(|| err(1, "empty recording".into()))?;
        let mut parts = magic.split_whitespace();
        if parts.next() != Some(FORMAT_NAME) {
            return Err(err(line, format!("not a {FORMAT_NAME} file")));
        }
        let version: u32 = parts
            .next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| err(line, "missing format version".into()))?;
        if version != FORMAT_VERSION {
            return Err(err(
                line,
                format!("unsupported format version {version}, expected {FORMAT_VERSION}"),
            ));
        }

        let mut state = PmoveState::new([0.0; 3]);
        state.stats = PmoveStats::default();
        let mut map = MapIdentity::default();
        let mut tick_rate_hz = 125;
        let mut touch_hook = true;
        let mut generator = String::new();
        let mut expected_ticks = None;
        let mut ticks = Vec::new();
        let mut in_ticks = false;

        for (line, text) in lines {
            if text.is_empty() || text.starts_with('#') {
                continue;
            }
            if in_ticks {
                ticks.push(parse_tick(text).map_err(|m| err(line, m))?);
                continue;
            }
            let (key, value) = text.split_once(' ').unwrap_or((text, ""));
            let value = value.trim();
            let bad = |what: &str| err(line, format!("invalid {what}: {value:?}"));
            match key {
                "map_name" => map.name = value.to_string(),
                "map_entry" => map.entry = value.to_string(),
                "map_size" => map.size = value.parse().map_err(|_| bad(key))?,
                "map_fnv1a64" => {
                    map.fnv1a64 = u64::from_str_radix(value, 16).map_err(|_| bad(key))?;
                }
                "tick_rate_hz" => tick_rate_hz = value.parse().map_err(|_| bad(key))?,
                "touch_hook" => touch_hook = value == "1",
                "generator" => generator = value.to_string(),
                "start_origin" => state.origin = vec3(value).ok_or_else(|| bad(key))?,
                "start_velocity" => state.velocity = vec3(value).ok_or_else(|| bad(key))?,
                "start_view_angles" => state.view_angles = vec3(value).ok_or_else(|| bad(key))?,
                "start_view_height" => state.view_height = value.parse().map_err(|_| bad(key))?,
                "start_mins" => state.mins = vec3(value).ok_or_else(|| bad(key))?,
                "start_maxs" => state.maxs = vec3(value).ok_or_else(|| bad(key))?,
                "start_flags" => state.flags = value.parse().map_err(|_| bad(key))?,
                "start_pm_time" => state.pm_time = value.parse().map_err(|_| bad(key))?,
                "start_gravity" => state.gravity = value.parse().map_err(|_| bad(key))?,
                "start_delta_angles" => {
                    let v: Vec<i16> = value
                        .split_whitespace()
                        .map(|p| p.parse().map_err(|_| bad(key)))
                        .collect::<Result<_, _>>()?;
                    state.delta_angles = v.try_into().map_err(|_| bad(key))?;
                }
                "ticks" => expected_ticks = Some(value.parse::<usize>().map_err(|_| bad(key))?),
                "tick_columns" => {
                    if value != TICK_COLUMNS {
                        return Err(err(line, "unexpected tick columns for this version".into()));
                    }
                    in_ticks = true;
                }
                _ => match key.strip_prefix("stat_") {
                    Some(name) => {
                        let parsed: i32 = value.parse().map_err(|_| bad(key))?;
                        if !set_stat(&mut state.stats, name, parsed) {
                            return Err(err(line, format!("unknown stat {name}")));
                        }
                    }
                    None => return Err(err(line, format!("unknown key {key}"))),
                },
            }
        }
        if expected_ticks != Some(ticks.len()) {
            return Err(err(
                0,
                format!(
                    "header declares {expected_ticks:?} ticks but {} rows were found",
                    ticks.len()
                ),
            ));
        }
        Ok(Self {
            map,
            tick_rate_hz,
            touch_hook,
            generator,
            initial: InitialState { state },
            ticks,
        })
    }
}

fn stat_fields(stats: &PmoveStats) -> [(&'static str, i32); 14] {
    [
        ("features", stats.features as i32),
        ("no_user_control", stats.no_user_control),
        ("knockback", stats.knockback),
        ("crouch_time", stats.crouch_time),
        ("zoom_time", stats.zoom_time),
        ("dash_time", stats.dash_time),
        ("wall_jump_time", stats.wall_jump_time),
        ("no_auto_attack", stats.no_auto_attack),
        ("stun", stats.stun),
        ("max_speed", stats.max_speed),
        ("jump_speed", stats.jump_speed),
        ("dash_speed", stats.dash_speed),
        ("forward_time", stats.forward_time),
        ("crouch_slide_time", stats.crouch_slide_time),
    ]
}

fn set_stat(stats: &mut PmoveStats, name: &str, value: i32) -> bool {
    match name {
        "features" => stats.features = value as u32,
        "no_user_control" => stats.no_user_control = value,
        "knockback" => stats.knockback = value,
        "crouch_time" => stats.crouch_time = value,
        "zoom_time" => stats.zoom_time = value,
        "dash_time" => stats.dash_time = value,
        "wall_jump_time" => stats.wall_jump_time = value,
        "no_auto_attack" => stats.no_auto_attack = value,
        "stun" => stats.stun = value,
        "max_speed" => stats.max_speed = value,
        "jump_speed" => stats.jump_speed = value,
        "dash_speed" => stats.dash_speed = value,
        "forward_time" => stats.forward_time = value,
        "crouch_slide_time" => stats.crouch_slide_time = value,
        _ => return false,
    }
    true
}

fn vec3(text: &str) -> Option<Vec3> {
    let values: Vec<f32> = text
        .split_whitespace()
        .map(|p| p.parse().ok().filter(|v: &f32| v.is_finite()))
        .collect::<Option<_>>()?;
    values.try_into().ok()
}

fn parse_tick(text: &str) -> Result<TickRecord, String> {
    let fields: Vec<&str> = text.split(',').collect();
    if fields.len() != TICK_COLUMNS.split(',').count() {
        return Err(format!("expected 29 columns, found {}", fields.len()));
    }
    fn num<T: std::str::FromStr>(fields: &[&str], index: usize) -> Result<T, String> {
        fields[index]
            .parse()
            .map_err(|_| format!("invalid column {index}: {:?}", fields[index]))
    }
    fn float(fields: &[&str], index: usize) -> Result<f32, String> {
        let value: f32 = num(fields, index)?;
        if value.is_finite() {
            Ok(value)
        } else {
            Err(format!("non-finite column {index}"))
        }
    }
    let pairs = |field: &str| -> Result<Vec<(i32, i32)>, String> {
        if field.is_empty() {
            return Ok(Vec::new());
        }
        field
            .split(';')
            .map(|item| {
                let (id, parameter) = item
                    .split_once(':')
                    .ok_or_else(|| format!("invalid event {item:?}"))?;
                Ok((
                    id.parse().map_err(|_| format!("invalid event {item:?}"))?,
                    parameter
                        .parse()
                        .map_err(|_| format!("invalid event {item:?}"))?,
                ))
            })
            .collect()
    };
    let touches = if fields[28].is_empty() {
        Vec::new()
    } else {
        fields[28]
            .split(';')
            .map(|t| t.parse().map_err(|_| format!("invalid touch {t:?}")))
            .collect::<Result<_, _>>()?
    };
    Ok(TickRecord {
        command: PmoveCommand {
            msec: num(&fields, 1)?,
            buttons: num(&fields, 2)?,
            forward_move: num(&fields, 3)?,
            side_move: num(&fields, 4)?,
            up_move: num(&fields, 5)?,
            angles: [num(&fields, 6)?, num(&fields, 7)?, num(&fields, 8)?],
        },
        result: TickResult {
            origin: [float(&fields, 9)?, float(&fields, 10)?, float(&fields, 11)?],
            velocity: [
                float(&fields, 12)?,
                float(&fields, 13)?,
                float(&fields, 14)?,
            ],
            grounded: num::<u8>(&fields, 15)? != 0,
            flags: num(&fields, 16)?,
            pm_time: num(&fields, 17)?,
            view_height: float(&fields, 18)?,
            maxs_z: float(&fields, 19)?,
            delta_angles: [num(&fields, 20)?, num(&fields, 21)?, num(&fields, 22)?],
            water_level: num(&fields, 23)?,
            ladder: num::<u8>(&fields, 24)? != 0,
            step: float(&fields, 25)?,
            trigger: num::<u8>(&fields, 26)? != 0,
            events: pairs(fields[27])?,
            touches,
        },
    })
}

/// One step of a scripted input sequence: hold `command` for `ticks` simulation ticks.
#[derive(Clone, Copy, Debug)]
pub struct ScriptStep {
    pub ticks: u32,
    pub command: WalkCommand,
}

/// The scripted session the sample recording is built from: settle, run, strafe-turn, jump,
/// crouch, dash, double jump and wall approach, with `yaw` as the initial heading.
pub fn sample_script(yaw: f32) -> Vec<ScriptStep> {
    let at = |yaw: f32| WalkCommand {
        view_angles: [0.0, yaw, 0.0],
        ..WalkCommand::default()
    };
    let step = |ticks, command| ScriptStep { ticks, command };
    let run = WalkCommand {
        forward: 1.0,
        ..at(yaw)
    };
    vec![
        step(100, at(yaw)),
        step(125, run),
        step(60, WalkCommand { right: 1.0, ..run }),
        step(
            50,
            WalkCommand {
                view_angles: [5.0, yaw + 30.0, 0.0],
                ..run
            },
        ),
        step(1, WalkCommand { up: 1.0, ..run }),
        step(
            80,
            WalkCommand {
                right: -1.0,
                view_angles: [0.0, yaw + 45.0, 0.0],
                ..run
            },
        ),
        step(60, WalkCommand { up: -1.0, ..run }),
        step(
            40,
            WalkCommand {
                up: -1.0,
                ..at(yaw + 45.0)
            },
        ),
        step(40, at(yaw + 45.0)),
        step(
            2,
            WalkCommand {
                special: true,
                forward: 1.0,
                ..at(yaw + 45.0)
            },
        ),
        step(
            60,
            WalkCommand {
                forward: 1.0,
                ..at(yaw + 45.0)
            },
        ),
        step(
            1,
            WalkCommand {
                up: 1.0,
                forward: 1.0,
                ..at(yaw + 90.0)
            },
        ),
        step(
            10,
            WalkCommand {
                forward: 1.0,
                ..at(yaw + 90.0)
            },
        ),
        step(
            2,
            WalkCommand {
                special: true,
                up: 1.0,
                forward: 1.0,
                ..at(yaw + 90.0)
            },
        ),
        step(
            120,
            WalkCommand {
                forward: 1.0,
                right: 1.0,
                ..at(yaw + 120.0)
            },
        ),
        step(
            125,
            WalkCommand {
                forward: -1.0,
                ..at(yaw + 120.0)
            },
        ),
    ]
}

/// Records a scripted run headlessly: walking from `spawn` with the given inputs, exactly as
/// the client's simulation would (no map entities, so plain `pmove`).
pub fn record_script<W: PmoveWorld>(
    world: &W,
    spawn: NoclipPlayer,
    map: MapIdentity,
    generator: &str,
    script: &[ScriptStep],
) -> Result<Recording, StepError> {
    let mut sim = ModeSimulation::new(spawn);
    sim.toggle_mode();
    let MovementMode::Walk(player) = sim.mode() else {
        unreachable!("toggled into walk mode")
    };
    let mut recorder = Recorder::start(map, generator, 125, false, &player.state);
    sim.set_tick_logging(true);
    for step in script {
        let elapsed = TICK_DURATION * step.ticks;
        sim.advance_frame(world, elapsed, step.command)?;
        for entry in sim.take_tick_log() {
            recorder.push(&entry);
        }
    }
    Ok(recorder.finish())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::pmove::{PmTrace, PmoveCommand};

    /// Infinite floor at z = 0.
    struct Floor;

    impl PmoveWorld for Floor {
        fn trace(&self, start: Vec3, mins: Vec3, _maxs: Vec3, end: Vec3, _mask: u32) -> PmTrace {
            let bottom_start = start[2] + mins[2];
            let bottom_end = end[2] + mins[2];
            if bottom_start >= 0.0 && bottom_end < 0.0 {
                let fraction = bottom_start / (bottom_start - bottom_end);
                let mut trace = PmTrace::miss(end);
                trace.fraction = fraction;
                trace.end_position =
                    std::array::from_fn(|i| start[i] + fraction * (end[i] - start[i]));
                trace.plane_normal = [0.0, 0.0, 1.0];
                trace.contents = 1;
                trace.entity = 0;
                trace
            } else {
                PmTrace::miss(end)
            }
        }

        fn point_contents(&self, _point: Vec3) -> u32 {
            0
        }
    }

    fn sample() -> Recording {
        let mut state = PmoveState::new([10.0, -20.5, 60.25]);
        state.stats.features = 0xffff;
        let mut recording = Recording {
            map: MapIdentity::of_bsp("floor", "maps/floor.bsp", b"bsp bytes"),
            tick_rate_hz: 125,
            touch_hook: true,
            generator: "test".into(),
            initial: InitialState { state },
            ticks: Vec::new(),
        };
        let mut live = state;
        for index in 0..120 {
            let command = PmoveCommand {
                msec: 8,
                buttons: 0,
                forward_move: if index > 40 { 127 } else { 0 },
                side_move: 0,
                up_move: if index == 60 { 127 } else { 0 },
                angles: [0, (index * 37) as i16, 0],
            };
            let result = pmove_with_touch(
                &Floor,
                &PmoveRules::default(),
                &mut live,
                &command,
                &mut NoTouch,
            );
            recording.ticks.push(TickRecord {
                command,
                result: TickResult::from_state(&live, &result, false),
            });
        }
        recording
    }

    #[test]
    fn text_round_trips_exactly() {
        let recording = sample();
        let text = recording.to_text();
        let parsed = Recording::from_text(&text).unwrap();
        assert_eq!(parsed, recording);
        assert_eq!(parsed.to_text(), text);
    }

    #[test]
    fn replay_is_deterministic_and_matches_recorded_results() {
        let recording = sample();
        let first = replay(&Floor, &PmoveRules::default(), &recording);
        let second = replay(&Floor, &PmoveRules::default(), &recording);
        assert_eq!(first, second);
        let recorded: Vec<_> = recording.ticks.iter().map(|t| t.result.clone()).collect();
        assert_eq!(first, recorded);
        assert!(
            first.iter().any(|t| t.events.iter().any(|e| e.0 == 13)),
            "jump event recorded"
        );
    }

    #[test]
    fn trigger_ticks_apply_the_recorded_state() {
        let mut recording = sample();
        recording.ticks[10].result.trigger = true;
        recording.ticks[10].result.velocity = [0.0, 0.0, 900.0];
        recording.ticks[10].result.flags |= crate::simulation::pmove::PMF_JUMPPAD_TIME;
        let replayed = replay(&Floor, &PmoveRules::default(), &recording);
        assert_eq!(replayed[10].velocity, [0.0, 0.0, 900.0]);
        assert!(replayed[11].origin[2] > replayed[10].origin[2]);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(Recording::from_text("").is_err());
        assert!(Recording::from_text("other 1\n").is_err());
        let future = sample().to_text().replacen(" 1\n", " 2\n", 1);
        let error = Recording::from_text(&future).unwrap_err();
        assert!(
            error.message.contains("unsupported format version"),
            "{error}"
        );
        let truncated = sample().to_text().replacen("ticks 120", "ticks 121", 1);
        assert!(Recording::from_text(&truncated).is_err());
    }

    #[test]
    fn map_identity_detects_a_different_bsp() {
        let identity = MapIdentity::of_bsp("m", "maps/m.bsp", b"abc");
        assert!(identity.matches(b"abc"));
        assert!(!identity.matches(b"abd"));
    }
}
