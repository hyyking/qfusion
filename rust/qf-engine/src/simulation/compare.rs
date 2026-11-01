//! Per-tick comparison of two movement traces (the Rust engine against the legacy
//! `gs_pmove.cpp` reference), divergence classification and report writers.
//!
//! Exact invariants (grounded state, flags, timers, water level, events, touched entities)
//! are compared separately from the floating-point trajectory (position, velocity, view
//! height, step height), which uses a tolerance. See `docs/movement-recording.md`.

use std::cell::{Cell, RefCell};
use std::fmt::{self, Write as _};

use super::pmove::{PmTrace, PmoveWorld, Vec3};
use super::recording::{Recording, TickResult};

/// Tolerances of the trajectory comparison. The default matches the legacy fixtures: the
/// original uses `rsqrtss` for dash normalisation, which leaves ~0.033 of error.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerances {
    pub position: f32,
    pub velocity: f32,
    pub view_height: f32,
    pub step: f32,
    /// Trace plane normals, only used to classify a divergence.
    pub normal: f32,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            position: 0.05,
            velocity: 0.05,
            view_height: 0.05,
            step: 0.05,
            normal: 0.01,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Position,
    Velocity,
    ViewHeight,
    MaxsZ,
    Step,
    Grounded,
    Flags,
    PmTime,
    WaterLevel,
    Ladder,
    Events,
    Touches,
}

impl Field {
    pub fn name(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::Velocity => "velocity",
            Self::ViewHeight => "view_height",
            Self::MaxsZ => "maxs_z",
            Self::Step => "step",
            Self::Grounded => "grounded",
            Self::Flags => "flags",
            Self::PmTime => "pm_time",
            Self::WaterLevel => "water_level",
            Self::Ladder => "ladder",
            Self::Events => "events",
            Self::Touches => "touches",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TickDiff {
    pub tick: usize,
    /// Largest absolute component difference.
    pub position_error: f32,
    pub velocity_error: f32,
    /// Fields outside tolerance (trajectory) or unequal (exact invariants).
    pub fields: Vec<Field>,
}

fn max_abs_diff(a: Vec3, b: Vec3) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max)
}

/// Compares one tick. `delta_angles` are not compared: the legacy movement code leaves them
/// to the game module.
pub fn diff_tick(
    tick: usize,
    rust: &TickResult,
    legacy: &TickResult,
    tol: &Tolerances,
) -> TickDiff {
    let position_error = max_abs_diff(rust.origin, legacy.origin);
    let velocity_error = max_abs_diff(rust.velocity, legacy.velocity);
    let mut fields = Vec::new();
    if position_error > tol.position {
        fields.push(Field::Position);
    }
    if velocity_error > tol.velocity {
        fields.push(Field::Velocity);
    }
    if (rust.view_height - legacy.view_height).abs() > tol.view_height {
        fields.push(Field::ViewHeight);
    }
    if rust.maxs_z != legacy.maxs_z {
        fields.push(Field::MaxsZ);
    }
    let step_differs =
        (rust.step - legacy.step).abs() > tol.step || (rust.step == 0.0) != (legacy.step == 0.0);
    if step_differs {
        fields.push(Field::Step);
    }
    if rust.grounded != legacy.grounded {
        fields.push(Field::Grounded);
    }
    if rust.flags != legacy.flags {
        fields.push(Field::Flags);
    }
    if rust.pm_time != legacy.pm_time {
        fields.push(Field::PmTime);
    }
    if rust.water_level != legacy.water_level {
        fields.push(Field::WaterLevel);
    }
    if rust.ladder != legacy.ladder {
        fields.push(Field::Ladder);
    }
    if rust.events != legacy.events {
        fields.push(Field::Events);
    }
    if rust.touches != legacy.touches {
        fields.push(Field::Touches);
    }
    TickDiff {
        tick,
        position_error,
        velocity_error,
        fields,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comparison {
    pub diffs: Vec<TickDiff>,
    /// Tick counts differ: the shorter side stopped early.
    pub length_mismatch: Option<(usize, usize)>,
}

impl Comparison {
    pub fn first_divergence(&self) -> Option<&TickDiff> {
        self.diffs.iter().find(|d| !d.fields.is_empty())
    }

    pub fn diverged_ticks(&self) -> usize {
        self.diffs.iter().filter(|d| !d.fields.is_empty()).count()
    }

    pub fn passed(&self) -> bool {
        self.length_mismatch.is_none() && self.first_divergence().is_none()
    }

    pub fn max_position_error(&self) -> f32 {
        self.diffs
            .iter()
            .map(|d| d.position_error)
            .fold(0.0, f32::max)
    }

    pub fn max_velocity_error(&self) -> f32 {
        self.diffs
            .iter()
            .map(|d| d.velocity_error)
            .fold(0.0, f32::max)
    }
}

pub fn compare(rust: &[TickResult], legacy: &[TickResult], tol: &Tolerances) -> Comparison {
    let diffs = rust
        .iter()
        .zip(legacy)
        .enumerate()
        .map(|(tick, (r, l))| diff_tick(tick, r, l, tol))
        .collect();
    Comparison {
        diffs,
        length_mismatch: (rust.len() != legacy.len()).then_some((rust.len(), legacy.len())),
    }
}

/// First tick where two result lists are not bit-for-bit identical.
pub fn first_exact_mismatch(a: &[TickResult], b: &[TickResult]) -> Option<usize> {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .or_else(|| (a.len() != b.len()).then_some(a.len().min(b.len())))
}

/// One logged collision query.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceCall {
    Box {
        start: Vec3,
        mins: Vec3,
        maxs: Vec3,
        end: Vec3,
        mask: u32,
        result: PmTrace,
    },
    Point {
        point: Vec3,
        contents: u32,
    },
}

/// Collision world wrapper that records every query for divergence analysis.
pub struct LoggingWorld<'a, W: PmoveWorld> {
    inner: &'a W,
    tick: Cell<usize>,
    log: RefCell<Vec<(usize, TraceCall)>>,
    window: (usize, usize),
}

impl<'a, W: PmoveWorld> LoggingWorld<'a, W> {
    /// Logs the queries of ticks in `first..=last`.
    pub fn new(inner: &'a W, first: usize, last: usize) -> Self {
        Self {
            inner,
            tick: Cell::new(0),
            log: RefCell::new(Vec::new()),
            window: (first, last),
        }
    }

    pub fn set_tick(&self, tick: usize) {
        self.tick.set(tick);
    }

    pub fn into_log(self) -> Vec<(usize, TraceCall)> {
        self.log.into_inner()
    }

    fn logging(&self) -> bool {
        (self.window.0..=self.window.1).contains(&self.tick.get())
    }
}

impl<W: PmoveWorld> PmoveWorld for LoggingWorld<'_, W> {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, mask: u32) -> PmTrace {
        let result = self.inner.trace(start, mins, maxs, end, mask);
        if self.logging() {
            self.log.borrow_mut().push((
                self.tick.get(),
                TraceCall::Box {
                    start,
                    mins,
                    maxs,
                    end,
                    mask,
                    result,
                },
            ));
        }
        result
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        let contents = self.inner.point_contents(point);
        if self.logging() {
            self.log
                .borrow_mut()
                .push((self.tick.get(), TraceCall::Point { point, contents }));
        }
        contents
    }

    fn is_player_entity(&self, entity: i32) -> bool {
        self.inner.is_player_entity(entity)
    }
}

/// Text form of the trace log, shared with the legacy `pmove_replay` tool:
///
/// ```text
/// T <tick> <start xyz> <mins xyz> <maxs xyz> <end xyz> <mask> <fraction> <endpos xyz>
///   <normal xyz> <startsolid> <allsolid> <surface flags> <contents> <entity>
/// P <tick> <point xyz> <contents>
/// ```
pub fn trace_log_to_text(log: &[(usize, TraceCall)]) -> String {
    let mut out = String::new();
    for (tick, call) in log {
        match call {
            TraceCall::Box {
                start,
                mins,
                maxs,
                end,
                mask,
                result: r,
            } => {
                let _ = writeln!(
                    out,
                    "T {tick} {} {} {} {} {mask} {} {} {} {} {} {} {} {}",
                    join(start),
                    join(mins),
                    join(maxs),
                    join(end),
                    r.fraction,
                    join(&r.end_position),
                    join(&r.plane_normal),
                    u8::from(r.start_solid),
                    u8::from(r.all_solid),
                    r.surface_flags,
                    r.contents,
                    r.entity,
                );
            }
            TraceCall::Point { point, contents } => {
                let _ = writeln!(out, "P {tick} {} {contents}", join(point));
            }
        }
    }
    out
}

fn join(v: &Vec3) -> String {
    format!("{} {} {}", v[0], v[1], v[2])
}

struct Tokens<'a> {
    it: std::str::SplitWhitespace<'a>,
    line: usize,
}

impl Tokens<'_> {
    fn parse<T: std::str::FromStr>(&mut self, what: &str) -> Result<T, String> {
        self.it
            .next()
            .and_then(|token| token.parse().ok())
            .ok_or_else(|| format!("trace log line {}: bad or missing {what}", self.line))
    }

    fn vec3(&mut self, what: &str) -> Result<Vec3, String> {
        Ok([self.parse(what)?, self.parse(what)?, self.parse(what)?])
    }
}

pub fn parse_trace_log(text: &str) -> Result<Vec<(usize, TraceCall)>, String> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut it = line.split_whitespace();
        let kind = it.next().unwrap_or("");
        let mut t = Tokens {
            it,
            line: index + 1,
        };
        let tick: usize = t.parse("tick")?;
        match kind {
            "T" => {
                let start = t.vec3("start")?;
                let mins = t.vec3("mins")?;
                let maxs = t.vec3("maxs")?;
                let end = t.vec3("end")?;
                let mask = t.parse("mask")?;
                let fraction = t.parse("fraction")?;
                let end_position = t.vec3("endpos")?;
                let plane_normal = t.vec3("normal")?;
                let start_solid = t.parse::<u8>("startsolid")? != 0;
                let all_solid = t.parse::<u8>("allsolid")? != 0;
                let surface_flags = t.parse("surface")?;
                let contents = t.parse("contents")?;
                let entity = t.parse("entity")?;
                out.push((
                    tick,
                    TraceCall::Box {
                        start,
                        mins,
                        maxs,
                        end,
                        mask,
                        result: PmTrace {
                            fraction,
                            end_position,
                            plane_normal,
                            surface_flags,
                            contents,
                            entity,
                            start_solid,
                            all_solid,
                        },
                    },
                ));
            }
            "P" => {
                let point = t.vec3("point")?;
                let contents = t.parse("contents")?;
                out.push((tick, TraceCall::Point { point, contents }));
            }
            _ => return Err(format!("trace log line {}: unknown record kind", index + 1)),
        }
    }
    Ok(out)
}

/// Why two traces first differ.
#[derive(Clone, Debug, PartialEq)]
pub enum Cause {
    /// Both sides asked the same collision question and got different answers.
    Collision { call: usize, detail: String },
    /// The collision queries differ, so the movement code took a different path.
    ControlFlow { call: usize, detail: String },
    /// Every collision query matched yet the state differs: arithmetic or bookkeeping.
    Integration { detail: String },
    /// No trace data available to decide.
    Unknown,
}

impl Cause {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Collision { .. } => "collision",
            Self::ControlFlow { .. } => "integration (control flow)",
            Self::Integration { .. } => "integration",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Collision { call, detail } => {
                write!(f, "collision mismatch at trace call #{call}: {detail}")
            }
            Self::ControlFlow { call, detail } => {
                write!(
                    f,
                    "movement took a different path at trace call #{call}: {detail}"
                )
            }
            Self::Integration { detail } => {
                write!(
                    f,
                    "identical collision queries, different integration: {detail}"
                )
            }
            Self::Unknown => write!(f, "no trace data to classify the divergence"),
        }
    }
}

fn near(a: Vec3, b: Vec3, tol: f32) -> bool {
    max_abs_diff(a, b) <= tol
}

/// Compares the collision calls of the divergence tick on both sides, in order.
pub fn classify(
    rust: &[TraceCall],
    legacy: &[TraceCall],
    diff: &TickDiff,
    tol: &Tolerances,
) -> Cause {
    if rust.is_empty() && legacy.is_empty() {
        return Cause::Unknown;
    }
    for (index, (r, l)) in rust.iter().zip(legacy).enumerate() {
        match (r, l) {
            (
                TraceCall::Box {
                    start: rs,
                    mins: rmin,
                    maxs: rmax,
                    end: re,
                    mask: rm,
                    result: rr,
                },
                TraceCall::Box {
                    start: ls,
                    mins: lmin,
                    maxs: lmax,
                    end: le,
                    mask: lm,
                    result: lr,
                },
            ) => {
                let same_query = near(*rs, *ls, tol.position)
                    && near(*re, *le, tol.position)
                    && near(*rmin, *lmin, tol.position)
                    && near(*rmax, *lmax, tol.position)
                    && rm == lm;
                if !same_query {
                    return Cause::ControlFlow {
                        call: index,
                        detail: format!(
                            "rust trace {rs:?}->{re:?} mask {rm:#x}, legacy {ls:?}->{le:?} mask {lm:#x}"
                        ),
                    };
                }
                let same_answer = rr.start_solid == lr.start_solid
                    && rr.all_solid == lr.all_solid
                    && (rr.fraction - lr.fraction).abs() * dist(*rs, *re) <= tol.position
                    && near(rr.end_position, lr.end_position, tol.position)
                    && (rr.fraction >= 1.0 && lr.fraction >= 1.0
                        || near(rr.plane_normal, lr.plane_normal, tol.normal)
                            && rr.surface_flags == lr.surface_flags
                            && rr.contents == lr.contents);
                if !same_answer {
                    return Cause::Collision {
                        call: index,
                        detail: format!(
                            "trace {rs:?}->{re:?}: rust fraction {} end {:?} normal {:?} solid {}/{}, legacy fraction {} end {:?} normal {:?} solid {}/{}",
                            rr.fraction,
                            rr.end_position,
                            rr.plane_normal,
                            rr.start_solid,
                            rr.all_solid,
                            lr.fraction,
                            lr.end_position,
                            lr.plane_normal,
                            lr.start_solid,
                            lr.all_solid
                        ),
                    };
                }
            }
            (
                TraceCall::Point {
                    point: rp,
                    contents: rc,
                },
                TraceCall::Point {
                    point: lp,
                    contents: lc,
                },
            ) => {
                if !near(*rp, *lp, tol.position) {
                    return Cause::ControlFlow {
                        call: index,
                        detail: format!("rust point {rp:?}, legacy point {lp:?}"),
                    };
                }
                if rc != lc {
                    return Cause::Collision {
                        call: index,
                        detail: format!("point contents at {rp:?}: rust {rc:#x}, legacy {lc:#x}"),
                    };
                }
            }
            _ => {
                return Cause::ControlFlow {
                    call: index,
                    detail: "different kinds of collision query".into(),
                };
            }
        }
    }
    if rust.len() != legacy.len() {
        return Cause::ControlFlow {
            call: rust.len().min(legacy.len()),
            detail: format!(
                "rust made {} collision queries, legacy {}",
                rust.len(),
                legacy.len()
            ),
        };
    }
    Cause::Integration {
        detail: diff
            .fields
            .iter()
            .map(|f| f.name())
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
}

/// Everything the report writers need.
pub struct Report<'a> {
    pub recording: &'a Recording,
    pub recording_name: &'a str,
    pub legacy_description: &'a str,
    pub rust: &'a [TickResult],
    pub legacy: &'a [TickResult],
    pub comparison: &'a Comparison,
    pub tolerances: &'a Tolerances,
    pub cause: Option<&'a Cause>,
    /// First tick where the Rust replay differs from the Rust results stored in the file.
    pub replay_mismatch: Option<usize>,
    pub trigger_ticks: usize,
    pub window: usize,
}

impl Report<'_> {
    pub fn passed(&self) -> bool {
        self.comparison.passed() && self.replay_mismatch.is_none()
    }

    pub fn to_markdown(&self) -> String {
        let c = self.comparison;
        let t = self.tolerances;
        let mut out = String::new();
        let _ = writeln!(out, "# Movement comparison: {}\n", self.recording_name);
        let _ = writeln!(
            out,
            "**Result: {}**\n",
            if self.passed() { "PASS" } else { "FAIL" }
        );
        let _ = writeln!(out, "| | |\n|---|---|");
        let _ = writeln!(
            out,
            "| Map | {} (`{}`) |",
            self.recording.map.name, self.recording.map.entry
        );
        let _ = writeln!(
            out,
            "| Map FNV-1a64 | `{:016x}` |",
            self.recording.map.fnv1a64
        );
        let _ = writeln!(out, "| Recorded by | {} |", self.recording.generator);
        let _ = writeln!(
            out,
            "| Ticks | {} at {} Hz |",
            self.recording.ticks.len(),
            self.recording.tick_rate_hz
        );
        let _ = writeln!(out, "| Reference | {} |", self.legacy_description);
        let _ = writeln!(
            out,
            "| Tolerances | position {} / velocity {} / view height {} / step {} units; flags, timers, grounded, events and touches exact |",
            t.position, t.velocity, t.view_height, t.step
        );
        let _ = writeln!(
            out,
            "| Trigger ticks resynced to the recording | {} |\n",
            self.trigger_ticks
        );

        let _ = writeln!(out, "## Rust replay determinism\n");
        match self.replay_mismatch {
            None => {
                let _ = writeln!(
                    out,
                    "The headless Rust replay reproduces the recorded per-tick state bit for bit.\n"
                );
            }
            Some(tick) => {
                let _ = writeln!(
                    out,
                    "**The Rust replay differs from the recorded Rust state at tick {tick}.** The recording was made by a different engine build or is corrupt; the legacy comparison below is against the replay.\n"
                );
            }
        }

        let _ = writeln!(out, "## Rust vs legacy\n");
        if let Some((rust, legacy)) = c.length_mismatch {
            let _ = writeln!(
                out,
                "**Tick count mismatch:** rust {rust}, legacy {legacy}.\n"
            );
        }
        match c.first_divergence() {
            None => {
                let _ = writeln!(
                    out,
                    "No divergence over {} ticks. Largest position error {:.6}, largest velocity error {:.6}.\n",
                    c.diffs.len(),
                    c.max_position_error(),
                    c.max_velocity_error()
                );
            }
            Some(first) => {
                let names: Vec<_> = first.fields.iter().map(|f| f.name()).collect();
                let i = first.tick;
                let _ = writeln!(
                    out,
                    "**First divergence at tick {i}** ({}): {}.\n",
                    self.recording_tick_time(i),
                    names.join(", ")
                );
                let _ = writeln!(out, "| | Rust | Legacy |\n|---|---|---|");
                let _ = writeln!(
                    out,
                    "| position | {:?} | {:?} |",
                    self.rust[i].origin, self.legacy[i].origin
                );
                let _ = writeln!(
                    out,
                    "| velocity | {:?} | {:?} |",
                    self.rust[i].velocity, self.legacy[i].velocity
                );
                let _ = writeln!(
                    out,
                    "| grounded | {} | {} |",
                    self.rust[i].grounded, self.legacy[i].grounded
                );
                let _ = writeln!(
                    out,
                    "| flags | {:#x} | {:#x} |",
                    self.rust[i].flags, self.legacy[i].flags
                );
                let _ = writeln!(
                    out,
                    "| events | {:?} | {:?} |\n",
                    self.rust[i].events, self.legacy[i].events
                );
                let _ = writeln!(
                    out,
                    "Ticks outside tolerance: {} of {}. Largest position error {:.4}, velocity error {:.4}.\n",
                    c.diverged_ticks(),
                    c.diffs.len(),
                    c.max_position_error(),
                    c.max_velocity_error()
                );
                let _ = writeln!(out, "### Cause\n");
                match self.cause {
                    Some(cause) => {
                        let _ = writeln!(out, "**{}**: {cause}\n", cause.kind());
                    }
                    None => {
                        let _ = writeln!(
                            out,
                            "Not classified (the legacy tool did not supply a trace log).\n"
                        );
                    }
                }
                let _ = writeln!(out, "### Error over time\n");
                let _ = writeln!(
                    out,
                    "| ticks | max position error | max velocity error | ticks out of tolerance |\n|---|---|---|---|"
                );
                for (index, chunk) in c.diffs.chunks(self.window.max(1)).enumerate() {
                    let from = index * self.window.max(1);
                    let to = from + chunk.len() - 1;
                    let pos = chunk.iter().map(|d| d.position_error).fold(0.0, f32::max);
                    let vel = chunk.iter().map(|d| d.velocity_error).fold(0.0, f32::max);
                    let bad = chunk.iter().filter(|d| !d.fields.is_empty()).count();
                    let _ = writeln!(out, "| {from}-{to} | {pos:.4} | {vel:.4} | {bad} |");
                }
                out.push('\n');
            }
        }
        out
    }

    fn recording_tick_time(&self, tick: usize) -> String {
        let hz = self.recording.tick_rate_hz.max(1) as f32;
        format!("t = {:.3} s", (tick + 1) as f32 / hz)
    }

    pub fn to_csv(&self) -> String {
        let mut out = String::from(
            "tick,rust_x,rust_y,rust_z,legacy_x,legacy_y,legacy_z,rust_vx,rust_vy,rust_vz,legacy_vx,legacy_vy,legacy_vz,position_error,velocity_error,rust_grounded,legacy_grounded,mismatch\n",
        );
        for diff in &self.comparison.diffs {
            let (r, l) = (&self.rust[diff.tick], &self.legacy[diff.tick]);
            let names: Vec<_> = diff.fields.iter().map(|f| f.name()).collect();
            let _ = writeln!(
                out,
                "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                diff.tick,
                r.origin[0],
                r.origin[1],
                r.origin[2],
                l.origin[0],
                l.origin[1],
                l.origin[2],
                r.velocity[0],
                r.velocity[1],
                r.velocity[2],
                l.velocity[0],
                l.velocity[1],
                l.velocity[2],
                diff.position_error,
                diff.velocity_error,
                u8::from(r.grounded),
                u8::from(l.grounded),
                names.join(";"),
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(x: f32) -> TickResult {
        TickResult {
            origin: [x, 0.0, 24.0],
            velocity: [x, 0.0, 0.0],
            grounded: true,
            flags: 4,
            pm_time: 0,
            view_height: 22.0,
            maxs_z: 40.0,
            delta_angles: [0; 3],
            water_level: 0,
            ladder: false,
            step: 0.0,
            trigger: false,
            events: Vec::new(),
            touches: Vec::new(),
        }
    }

    #[test]
    fn identical_traces_pass_and_small_errors_stay_in_tolerance() {
        let a: Vec<_> = (0..10).map(|i| result(i as f32)).collect();
        let mut b = a.clone();
        b[4].origin[0] += 0.03;
        let comparison = compare(&a, &b, &Tolerances::default());
        assert!(comparison.passed());
        assert!((comparison.max_position_error() - 0.03).abs() < 1e-4);
    }

    #[test]
    fn reports_first_divergence_and_exact_field_mismatches() {
        let a: Vec<_> = (0..10).map(|i| result(i as f32)).collect();
        let mut b = a.clone();
        b[3].origin[0] += 1.0;
        b[6].events.push((13, 0));
        b[7].grounded = false;
        let comparison = compare(&a, &b, &Tolerances::default());
        assert!(!comparison.passed());
        let first = comparison.first_divergence().unwrap();
        assert_eq!(first.tick, 3);
        assert_eq!(first.fields, vec![Field::Position]);
        assert_eq!(comparison.diffs[6].fields, vec![Field::Events]);
        assert_eq!(comparison.diffs[7].fields, vec![Field::Grounded]);
        assert_eq!(comparison.diverged_ticks(), 3);
    }

    #[test]
    fn length_mismatch_fails() {
        let a: Vec<_> = (0..5).map(|i| result(i as f32)).collect();
        let comparison = compare(&a, &a[..3], &Tolerances::default());
        assert!(!comparison.passed());
        assert_eq!(comparison.length_mismatch, Some((5, 3)));
    }

    fn call(fraction: f32, normal: Vec3) -> TraceCall {
        TraceCall::Box {
            start: [0.0, 0.0, 30.0],
            mins: [-16.0, -16.0, -24.0],
            maxs: [16.0, 16.0, 40.0],
            end: [0.0, 0.0, 20.0],
            mask: 3,
            result: PmTrace {
                fraction,
                end_position: [0.0, 0.0, 30.0 - 10.0 * fraction],
                plane_normal: normal,
                surface_flags: 0,
                contents: 1,
                entity: 0,
                start_solid: false,
                all_solid: false,
            },
        }
    }

    fn some_diff() -> TickDiff {
        TickDiff {
            tick: 5,
            position_error: 1.0,
            velocity_error: 0.0,
            fields: vec![Field::Position],
        }
    }

    #[test]
    fn classifies_collision_control_flow_and_integration() {
        let tol = Tolerances::default();
        let up = [0.0, 0.0, 1.0];
        let hit = vec![call(0.5, up)];
        assert_eq!(
            classify(&hit, &hit, &some_diff(), &tol),
            Cause::Integration {
                detail: "position".into()
            }
        );
        let other = vec![call(0.9, up)];
        assert!(matches!(
            classify(&hit, &other, &some_diff(), &tol),
            Cause::Collision { call: 0, .. }
        ));
        let extra = vec![call(0.5, up), call(1.0, up)];
        assert!(matches!(
            classify(&hit, &extra, &some_diff(), &tol),
            Cause::ControlFlow { call: 1, .. }
        ));
        assert_eq!(classify(&[], &[], &some_diff(), &tol), Cause::Unknown);
    }

    #[test]
    fn trace_log_round_trips() {
        let log = vec![
            (3, call(0.25, [0.0, 0.0, 1.0])),
            (
                3,
                TraceCall::Point {
                    point: [1.0, 2.0, 3.5],
                    contents: 8,
                },
            ),
        ];
        let parsed = parse_trace_log(&trace_log_to_text(&log)).unwrap();
        assert_eq!(parsed, log);
        assert!(parse_trace_log("X 1").is_err());
    }
}
