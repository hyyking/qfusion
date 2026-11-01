//! Brush movers: `func_door`, `func_door_rotating`, `func_plat`, `func_button`
//! and `func_rotating`, ported from `g_func.cpp` with the pusher of
//! `g_phys.cpp` (`SV_Physics_Pusher` / `SV_Push`).
//!
//! Movers are stepped once per 8 ms tick after the player's movement, in the
//! legacy order: for every team captain in entity order, the thinks of the
//! whole team run, then the team is pushed along its velocities. Only the local
//! player can be pushed, ridden on or crushed.

use bevy_ecs::prelude::*;

use crate::content::bsp::ModelPose;
use crate::content::bsp::entities::Entity as LumpEntity;
use crate::simulation::pmove::{
    CONTENTS_PLAYERCLIP, CONTENTS_SOLID, MASK_PLAYERSOLID, PmTrace, PmType, PmoveState, PmoveWorld,
    Vec3, angle_vectors,
};

use super::components::{Enabled, Level, LumpIndex, Registry, TouchKind, UseKind, Volume};
use super::logic;
use super::parse::{atof, atoi, entity_angles, movedir, seconds_to_ms};
use super::{BrushModels, MoverPose, TriggerEvent};

/// `game.frametime` in milliseconds: one tick.
const FRAME_MS: i64 = 8;
const FRAME_S: f32 = 0.008;
const DOOR_START_OPEN: i32 = 1;
const DOOR_CRUSHER: i32 = 4;
const DOOR_TOGGLE: i32 = 32;
const DOOR_X_AXIS: i32 = 64;
const DOOR_Y_AXIS: i32 = 128;
const DOOR_REVERSE: i32 = 2;
const PLAT_LOW_TRIGGER: i32 = 1;
const ROTATING_START_OFF: i32 = 1;
const ROTATING_REVERSE: i32 = 2;
const ROTATING_X_AXIS: i32 = 4;
const ROTATING_Y_AXIS: i32 = 8;
const ROTATING_STOP: i32 = 32;
const DOOR_TRIGGER_RETRIGGER_MS: i64 = 1000;
const DOOR_TRIGGER_MARGIN: f32 = 80.0;
const PLAT_TRIGGER_MARGIN: f32 = 25.0;
const PLAT_WAIT_MS: i64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MoverKind {
    Door,
    DoorRotating,
    Plat,
    Button,
    Rotating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum State {
    Bottom,
    Top,
    Up,
    Down,
    Stopped,
    Accel,
    FullSpeed,
    Decel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoverThink {
    MoveBegin,
    MoveWatch,
    MoveDone,
    AngleBegin,
    AngleWatch,
    AngleDone,
    DoorGoDown,
    PlatGoDown,
    ButtonReturn,
    RotateAccel,
    RotateDecel,
    SpawnDoorTrigger,
    CalcMoveSpeed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EndFunc {
    DoorHitTop,
    DoorHitBottom,
    PlatHitTop,
    PlatHitBottom,
    ButtonWait,
    ButtonDone,
}

/// `moveinfo` linear movement: position is a function of the server time.
#[derive(Clone, Copy, Debug)]
struct Linear {
    begin: Vec3,
    end: Vec3,
    timestamp: i64,
    duration: i64,
}

#[derive(Clone, Debug)]
pub(super) struct Mover {
    pub entity: Entity,
    pub lump: usize,
    pub kind: MoverKind,
    pub model: usize,
    pub origin: Vec3,
    pub angles: Vec3,
    /// Local bounds of the brush model.
    mins: Vec3,
    maxs: Vec3,
    velocity: Vec3,
    avelocity: Vec3,
    linear: Option<Linear>,
    pub state: State,
    speed: f32,
    wait_s: f32,
    movedir: Vec3,
    distance: f32,
    start_origin: Vec3,
    end_origin: Vec3,
    start_angles: Vec3,
    end_angles: Vec3,
    dest: Vec3,
    dest_angles: Vec3,
    end_func: Option<EndFunc>,
    think: Option<(i64, MoverThink)>,
    spawnflags: i32,
    dmg: i32,
    health: i32,
    max_health: i32,
    accel: f32,
    decel: f32,
    current_speed: f32,
    team: Option<String>,
    leader: usize,
    next: Option<usize>,
    follower: bool,
    pushes: bool,
    trigger_stamp: i64,
    touchable: bool,
}

/// All movers in lump order, plus the team captain whose frame is running.
#[derive(Resource, Default)]
pub(super) struct Movers {
    pub list: Vec<Mover>,
    current: Option<usize>,
}

fn mv(world: &mut World, index: usize) -> &mut Mover {
    &mut world.resource_mut::<Movers>().into_inner().list[index]
}

fn now(world: &World) -> i64 {
    world.resource::<Level>().time_ms
}

fn team_of(world: &World, index: usize) -> Vec<usize> {
    let movers = world.resource::<Movers>();
    let mut chain = vec![movers.list[index].leader];
    while let Some(next) = movers.list[*chain.last().unwrap()].next {
        chain.push(next);
    }
    chain
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] + b[i])
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}

fn scale(a: Vec3, s: f32) -> Vec3 {
    a.map(|v| v * s)
}

fn length(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `VectorNormalize`: returns the unit vector (zero stays zero) and the length.
fn normalize(a: Vec3) -> (Vec3, f32) {
    let len = length(a);
    if len > 0.0 {
        (scale(a, 1.0 / len), len)
    } else {
        ([0.0; 3], 0.0)
    }
}

fn is_zero(a: Vec3) -> bool {
    a == [0.0; 3]
}

/// `AnglesToAxis`: forward, left, up.
pub(super) fn axis_rows(angles: Vec3) -> [Vec3; 3] {
    let (forward, right, up) = angle_vectors(angles);
    [forward, right.map(|v| -v), up]
}

pub(super) fn pose_of(pose: &MoverPose) -> ModelPose {
    ModelPose {
        origin: pose.origin,
        axis: (!is_zero(pose.angles)).then(|| axis_rows(pose.angles)),
    }
}

/// `GClip_LinkEntity` bounds: rotated models use a sphere around the origin.
fn abs_bounds(origin: Vec3, angles: Vec3, mins: Vec3, maxs: Vec3) -> (Vec3, Vec3) {
    if is_zero(angles) {
        (
            std::array::from_fn(|i| origin[i] + mins[i] - 1.0),
            std::array::from_fn(|i| origin[i] + maxs[i] + 1.0),
        )
    } else {
        let radius = length(std::array::from_fn(|i| mins[i].abs().max(maxs[i].abs())));
        (
            origin.map(|v| v - radius - 1.0),
            origin.map(|v| v + radius + 1.0),
        )
    }
}

/// Movement collision with movers added to a static world: every hit by a
/// mover reports its lump index as the entity number.
pub struct MoverWorld<'a> {
    world: &'a dyn PmoveWorld,
    models: &'a dyn BrushModels,
    poses: Vec<MoverPose>,
}

impl<'a> MoverWorld<'a> {
    pub fn new(
        world: &'a dyn PmoveWorld,
        models: &'a dyn BrushModels,
        poses: Vec<MoverPose>,
    ) -> Self {
        Self {
            world,
            models,
            poses,
        }
    }
}

impl PmoveWorld for MoverWorld<'_> {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, mask: u32) -> PmTrace {
        let mut best = self.world.trace(start, mins, maxs, end, mask);
        for pose in &self.poses {
            let Some(trace) =
                self.models
                    .trace_model(pose.model, &pose_of(pose), (start, end, mins, maxs), mask)
            else {
                continue;
            };
            if trace.all_solid || trace.fraction < best.fraction {
                best = PmTrace {
                    fraction: trace.fraction,
                    end_position: trace.end_position,
                    plane_normal: trace.plane.map_or([0.0; 3], |plane| plane.normal),
                    surface_flags: trace.surface_flags,
                    contents: trace.contents,
                    entity: pose.entity as i32,
                    start_solid: trace.start_solid,
                    all_solid: trace.all_solid,
                };
            }
        }
        best
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        let mut contents = self.world.point_contents(point);
        for pose in &self.poses {
            if let Some(trace) = self.models.trace_model(
                pose.model,
                &pose_of(pose),
                (point, point, [0.0; 3], [0.0; 3]),
                CONTENTS_SOLID | CONTENTS_PLAYERCLIP,
            ) && trace.start_solid
            {
                contents |= trace.contents;
            }
        }
        contents
    }
}

/// The local player as seen by the pushers.
pub struct PushedPlayer<'a> {
    pub state: &'a mut PmoveState,
    /// Entity standing on after the last movement command.
    pub ground_entity: Option<i32>,
}

struct Ctx<'a, 'b> {
    models: &'a dyn BrushModels,
    world: &'a dyn PmoveWorld,
    player: Option<PushedPlayer<'b>>,
}

pub(super) fn poses(world: &World) -> Vec<MoverPose> {
    world
        .resource::<Movers>()
        .list
        .iter()
        .map(|m| MoverPose {
            entity: m.lump,
            model: m.model,
            origin: m.origin,
            angles: m.angles,
        })
        .collect()
}

// ---------------------------------------------------------------- spawning

fn model_of(entity: &LumpEntity) -> Option<usize> {
    entity
        .get("model")?
        .strip_prefix('*')?
        .parse::<usize>()
        .ok()
        .filter(|&model| model != 0)
}

fn key(entity: &LumpEntity, name: &str) -> f32 {
    entity.get(name).map_or(0.0, atof)
}

fn or_default(value: f32, default: f32) -> f32 {
    if value != 0.0 { value } else { default }
}

pub(super) fn spawn_all(
    world: &mut World,
    lump: &[LumpEntity],
    models: &(impl BrushModels + ?Sized),
) {
    world.insert_resource(Movers::default());
    let mut movers = Vec::new();
    for (index, entity) in lump.iter().enumerate() {
        let Some(Some(ecs)) = world.resource::<Registry>().by_lump.get(index).copied() else {
            continue;
        };
        let classname = entity
            .get("classname")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let kind = match classname.as_str() {
            "func_door" => MoverKind::Door,
            "func_door_rotating" => MoverKind::DoorRotating,
            "func_plat" => MoverKind::Plat,
            "func_button" => MoverKind::Button,
            "func_rotating" => MoverKind::Rotating,
            _ => continue,
        };
        let Some(model) = model_of(entity) else {
            continue;
        };
        let Some((mins, maxs)) = models.model_bounds(model) else {
            continue;
        };
        movers.push(build(ecs, index, kind, model, mins, maxs, entity));
    }
    link_teams(&mut movers);
    let count = movers.len();
    for (i, mover) in movers.iter().enumerate() {
        world.entity_mut(mover.entity).insert(UseKind::Mover(i));
    }
    world.resource_mut::<Movers>().list = movers;

    for i in 0..count {
        let (kind, spawnflags, start_open_use) = {
            let m = &world.resource::<Movers>().list[i];
            (
                m.kind,
                m.spawnflags,
                m.kind == MoverKind::Rotating && m.spawnflags & ROTATING_START_OFF == 0,
            )
        };
        if kind == MoverKind::Plat {
            spawn_plat_trigger(world, i, spawnflags);
        }
        if start_open_use {
            use_mover(world, i);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    entity: Entity,
    lump: usize,
    kind: MoverKind,
    model: usize,
    mins: Vec3,
    maxs: Vec3,
    source: &LumpEntity,
) -> Mover {
    let spawnflags = source.get("spawnflags").map_or(0, atoi);
    let origin = source
        .get("origin")
        .and_then(super::parse::parse_vec3)
        .unwrap_or([0.0; 3]);
    let size = sub(maxs, mins);
    let dmg = or_default(key(source, "dmg"), 2.0) as i32;
    let mut m = Mover {
        entity,
        lump,
        kind,
        model,
        origin,
        angles: [0.0; 3],
        mins,
        maxs,
        velocity: [0.0; 3],
        avelocity: [0.0; 3],
        linear: None,
        state: State::Bottom,
        speed: 0.0,
        wait_s: key(source, "wait"),
        movedir: [0.0; 3],
        distance: 0.0,
        start_origin: origin,
        end_origin: origin,
        start_angles: [0.0; 3],
        end_angles: [0.0; 3],
        dest: origin,
        dest_angles: [0.0; 3],
        end_func: None,
        think: None,
        spawnflags,
        dmg,
        health: 0,
        max_health: 0,
        accel: 0.0,
        decel: 0.0,
        current_speed: 0.0,
        team: source.get("team").map(str::to_owned),
        leader: 0,
        next: None,
        follower: false,
        pushes: true,
        trigger_stamp: i64::MIN / 2,
        touchable: false,
    };
    let health = source.get("health").map_or(0, atoi);
    let lip = |default: f32| or_default(key(source, "lip"), default);
    match kind {
        MoverKind::Door => {
            m.movedir = movedir(source);
            m.speed = or_default(key(source, "speed"), 600.0);
            m.wait_s = or_default(m.wait_s, 2.0);
            if health > 0 {
                m.health = health;
                m.max_health = health;
            }
            m.distance = (0..3).map(|i| m.movedir[i].abs() * size[i]).sum::<f32>() - lip(8.0);
            m.start_origin = origin;
            m.end_origin = add(origin, scale(m.movedir, m.distance));
            if spawnflags & DOOR_START_OPEN != 0 {
                m.origin = m.end_origin;
                std::mem::swap(&mut m.start_origin, &mut m.end_origin);
                m.movedir = scale(m.movedir, -1.0);
            }
            m.think = Some((
                1,
                if m.health > 0 || source.get("targetname").is_some() {
                    MoverThink::CalcMoveSpeed
                } else {
                    MoverThink::SpawnDoorTrigger
                },
            ));
        }
        MoverKind::DoorRotating => {
            m.speed = or_default(key(source, "speed"), 100.0);
            m.wait_s = or_default(m.wait_s, 3.0);
            m.movedir = if spawnflags & DOOR_X_AXIS != 0 {
                [0.0, 0.0, 1.0]
            } else if spawnflags & DOOR_Y_AXIS != 0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            if spawnflags & DOOR_REVERSE != 0 {
                m.movedir = scale(m.movedir, -1.0);
            }
            m.distance = or_default(key(source, "distance"), 90.0);
            if health > 0 {
                m.health = health;
                m.max_health = health;
            }
            m.start_angles = [0.0; 3];
            m.end_angles = scale(m.movedir, m.distance);
            if spawnflags & DOOR_START_OPEN != 0 {
                m.angles = m.end_angles;
                std::mem::swap(&mut m.start_angles, &mut m.end_angles);
                m.movedir = scale(m.movedir, -1.0);
            }
            m.think = Some((
                1,
                if m.health > 0 || source.get("targetname").is_some() {
                    MoverThink::CalcMoveSpeed
                } else {
                    MoverThink::SpawnDoorTrigger
                },
            ));
        }
        MoverKind::Plat => {
            m.speed = or_default(key(source, "speed"), 300.0);
            let height = key(source, "height");
            m.start_origin = origin;
            m.end_origin = origin;
            m.end_origin[2] -= if height != 0.0 {
                height
            } else {
                size[2] - lip(8.0)
            };
            m.distance = lip(8.0);
            if source.get("targetname").is_some() {
                m.state = State::Top;
            } else {
                m.origin = m.end_origin;
                m.state = State::Bottom;
            }
        }
        MoverKind::Button => {
            m.movedir = movedir(source);
            m.speed = or_default(key(source, "speed"), 40.0);
            m.wait_s = or_default(m.wait_s, 3.0);
            m.distance = (0..3).map(|i| m.movedir[i].abs() * size[i]).sum::<f32>() - lip(4.0);
            m.start_origin = origin;
            m.end_origin = add(origin, scale(m.movedir, m.distance));
            if health > 0 {
                m.health = health;
                m.max_health = health;
            } else if source.get("targetname").is_none() {
                m.touchable = true;
            }
        }
        MoverKind::Rotating => {
            m.angles = entity_angles(source);
            m.pushes = spawnflags & ROTATING_STOP == 0;
            m.state = State::Stopped;
            m.speed = or_default(key(source, "speed"), 100.0);
            m.movedir = if spawnflags & ROTATING_X_AXIS != 0 {
                [0.0, 0.0, 1.0]
            } else if spawnflags & ROTATING_Y_AXIS != 0 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            };
            if spawnflags & ROTATING_REVERSE != 0 {
                m.movedir = scale(m.movedir, -1.0);
            }
            let rate = |name| {
                let value = key(source, name);
                if value < 0.0 { 0.0 } else { value * 0.1 }
            };
            m.accel = rate("accel");
            m.decel = rate("decel");
        }
    }
    m
}

fn link_teams(movers: &mut [Mover]) {
    let mut linked = vec![false; movers.len()];
    for i in 0..movers.len() {
        movers[i].leader = i;
        if linked[i] || movers[i].team.is_none() {
            continue;
        }
        let mut tail = i;
        for j in i + 1..movers.len() {
            if !linked[j] && movers[j].team == movers[i].team {
                linked[j] = true;
                movers[j].leader = i;
                movers[j].follower = true;
                movers[tail].next = Some(j);
                tail = j;
            }
        }
    }
}

fn spawn_plat_trigger(world: &mut World, index: usize, spawnflags: i32) {
    let m = &world.resource::<Movers>().list[index];
    let (lump, mins, maxs, lip) = (m.lump, m.mins, m.maxs, m.distance);
    let travel = (m.start_origin[2] - m.end_origin[2]).max(0.0);
    let mut tmin = [
        mins[0] + PLAT_TRIGGER_MARGIN,
        mins[1] + PLAT_TRIGGER_MARGIN,
        0.0,
    ];
    let mut tmax = [
        maxs[0] - PLAT_TRIGGER_MARGIN,
        maxs[1] - PLAT_TRIGGER_MARGIN,
        maxs[2] + 8.0,
    ];
    tmin[2] = tmax[2] - (travel + lip);
    if spawnflags & PLAT_LOW_TRIGGER != 0 {
        tmax[2] = tmin[2] + 8.0;
    }
    for axis in 0..2 {
        if tmax[axis] - tmin[axis] <= 0.0 {
            tmin[axis] = (mins[axis] + maxs[axis]) * 0.5;
            tmax[axis] = tmin[axis] + 1.0;
        }
    }
    spawn_trigger(world, lump, index, tmin, tmax, true);
}

fn spawn_trigger(world: &mut World, lump: usize, owner: usize, mins: Vec3, maxs: Vec3, plat: bool) {
    world.spawn((
        LumpIndex(lump),
        Volume {
            model: 0,
            origin: [0.0; 3],
            mins,
            maxs,
            absmin: mins.map(|v| v - 1.0),
            absmax: maxs.map(|v| v + 1.0),
        },
        Enabled(true),
        TouchKind::MoverTrigger { owner, plat },
    ));
}

// ------------------------------------------------------------------- moves

fn move_calc(world: &mut World, i: usize, dest: Vec3, end: EndFunc) {
    let time = now(world);
    let current = world.resource::<Movers>().current;
    let m = mv(world, i);
    m.velocity = [0.0; 3];
    m.dest = dest;
    m.end_func = Some(end);
    m.linear = None;
    let captain = if m.follower { m.leader } else { i };
    if current == Some(captain) {
        move_begin(world, i);
    } else {
        mv(world, i).think = Some((time + 1, MoverThink::MoveBegin));
    }
}

fn move_begin(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    let (dir, dist) = normalize(sub(m.dest, m.origin));
    m.velocity = scale(dir, m.speed);
    let duration = if m.speed > 0.0 {
        ((dist * 1000.0 / m.speed) as i64).max(1)
    } else {
        1
    };
    m.linear = Some(Linear {
        begin: m.origin,
        end: m.dest,
        timestamp: time - FRAME_MS,
        duration,
    });
    m.think = Some((time + 1, MoverThink::MoveWatch));
}

fn move_watch(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    let done = m.linear.is_none_or(|l| time - l.timestamp >= l.duration);
    m.think = Some((
        time + 1,
        if done {
            MoverThink::MoveDone
        } else {
            MoverThink::MoveWatch
        },
    ));
}

fn move_done(world: &mut World, i: usize) {
    let m = mv(world, i);
    m.velocity = [0.0; 3];
    if let Some(end) = m.end_func.take() {
        end_func(world, i, end);
    }
}

fn linear_position(linear: &Linear, time: i64) -> Vec3 {
    let move_time = (time - linear.timestamp).max(0).min(linear.duration);
    if move_time >= linear.duration {
        return linear.end;
    }
    let fraction = move_time as f32 / linear.duration as f32;
    add(linear.begin, scale(sub(linear.end, linear.begin), fraction))
}

fn angle_calc(world: &mut World, i: usize, dest: Vec3, end: EndFunc) {
    let time = now(world);
    let current = world.resource::<Movers>().current;
    let m = mv(world, i);
    m.avelocity = [0.0; 3];
    m.dest_angles = dest;
    m.end_func = Some(end);
    let captain = if m.follower { m.leader } else { i };
    if current == Some(captain) {
        angle_begin(world, i);
    } else {
        mv(world, i).think = Some((time + 1, MoverThink::AngleBegin));
    }
}

/// `AngleMove_AdjustFinalStep`. The final step covers exactly the remaining
/// angle (the legacy step is a unit vector, which overshoots).
fn angle_final_step(m: &mut Mover) -> bool {
    let delta = sub(m.dest_angles, m.angles);
    let remaining = length(delta);
    if remaining <= m.speed * FRAME_S {
        m.avelocity = scale(delta, 1.0 / FRAME_S);
        return true;
    }
    false
}

fn angle_begin(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    if angle_final_step(m) {
        m.think = Some((time + 1, MoverThink::AngleDone));
        return;
    }
    let (dir, _) = normalize(sub(m.dest_angles, m.angles));
    m.avelocity = scale(dir, m.speed);
    m.think = Some((time + 1, MoverThink::AngleWatch));
}

fn angle_watch(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    let (dir, _) = normalize(sub(m.dest_angles, m.angles));
    if is_zero(dir) {
        return angle_done(world, i);
    }
    if angle_final_step(m) {
        m.think = Some((time + 1, MoverThink::AngleDone));
        return;
    }
    m.avelocity = scale(dir, m.speed);
    m.think = Some((time + 1, MoverThink::AngleWatch));
}

fn angle_done(world: &mut World, i: usize) {
    let m = mv(world, i);
    m.avelocity = [0.0; 3];
    m.angles = m.dest_angles;
    if let Some(end) = m.end_func.take() {
        end_func(world, i, end);
    }
}

fn end_func(world: &mut World, i: usize, end: EndFunc) {
    let time = now(world);
    let m = mv(world, i);
    match end {
        EndFunc::DoorHitTop => {
            m.state = State::Top;
            if m.spawnflags & DOOR_TOGGLE != 0 {
                return;
            }
            if m.wait_s >= 0.0 {
                m.think = Some((time + seconds_to_ms(m.wait_s), MoverThink::DoorGoDown));
            }
        }
        EndFunc::DoorHitBottom | EndFunc::PlatHitBottom | EndFunc::ButtonDone => {
            m.state = State::Bottom;
        }
        EndFunc::PlatHitTop => {
            m.state = State::Top;
            m.think = Some((time + PLAT_WAIT_MS, MoverThink::PlatGoDown));
        }
        EndFunc::ButtonWait => {
            m.state = State::Top;
            let (entity, wait_s) = (m.entity, m.wait_s);
            logic::use_targets(world, entity);
            if wait_s >= 0.0 {
                mv(world, i).think = Some((time + seconds_to_ms(wait_s), MoverThink::ButtonReturn));
            }
        }
    }
}

// ------------------------------------------------------------------- thinks

fn run_think(world: &mut World, i: usize) {
    let time = now(world);
    let Some((due, kind)) = mv(world, i).think else {
        return;
    };
    if due > time {
        return;
    }
    mv(world, i).think = None;
    match kind {
        MoverThink::MoveBegin => move_begin(world, i),
        MoverThink::MoveWatch => move_watch(world, i),
        MoverThink::MoveDone => move_done(world, i),
        MoverThink::AngleBegin => angle_begin(world, i),
        MoverThink::AngleWatch => angle_watch(world, i),
        MoverThink::AngleDone => angle_done(world, i),
        MoverThink::DoorGoDown => door_go_down(world, i),
        MoverThink::PlatGoDown => plat_go_down(world, i),
        MoverThink::ButtonReturn => {
            mv(world, i).state = State::Down;
            let dest = mv(world, i).start_origin;
            move_calc(world, i, dest, EndFunc::ButtonDone);
        }
        MoverThink::RotateAccel => rotate_think(world, i, true),
        MoverThink::RotateDecel => rotate_think(world, i, false),
        MoverThink::SpawnDoorTrigger => spawn_door_trigger(world, i),
        MoverThink::CalcMoveSpeed => calc_move_speed(world, i),
    }
}

/// `Think_CalcMoveSpeed`: all doors of a team arrive together.
fn calc_move_speed(world: &mut World, i: usize) {
    if mv(world, i).follower {
        return;
    }
    let team = team_of(world, i);
    let shortest = team
        .iter()
        .map(|&part| mv(world, part).distance.abs())
        .fold(f32::INFINITY, f32::min);
    for part in team {
        let m = mv(world, part);
        let time = shortest / m.speed;
        if time > 0.0 {
            m.speed = m.distance.abs() / time;
        }
    }
}

/// `Think_SpawnDoorTrigger`.
fn spawn_door_trigger(world: &mut World, i: usize) {
    if mv(world, i).follower {
        return;
    }
    let team = team_of(world, i);
    let mut mins = [f32::INFINITY; 3];
    let mut maxs = [f32::NEG_INFINITY; 3];
    for &part in &team {
        let m = mv(world, part);
        let (lo, hi) = abs_bounds(m.origin, m.angles, m.mins, m.maxs);
        for axis in 0..3 {
            mins[axis] = mins[axis].min(lo[axis]);
            maxs[axis] = maxs[axis].max(hi[axis]);
        }
    }
    for axis in 0..2 {
        mins[axis] -= DOOR_TRIGGER_MARGIN;
        maxs[axis] += DOOR_TRIGGER_MARGIN;
    }
    let lump = mv(world, i).lump;
    spawn_trigger(world, lump, i, mins, maxs, false);
    calc_move_speed(world, i);
}

// ---------------------------------------------------------------- behaviour

/// `ent->use` of every mover.
pub(super) fn use_mover(world: &mut World, i: usize) {
    match mv(world, i).kind {
        MoverKind::Door | MoverKind::DoorRotating => door_use(world, i),
        MoverKind::Plat => {
            if mv(world, i).think.is_none() {
                plat_go_down(world, i);
            }
        }
        MoverKind::Button => button_fire(world, i),
        MoverKind::Rotating => rotating_use(world, i),
    }
}

fn door_use(world: &mut World, i: usize) {
    let m = mv(world, i);
    if m.follower {
        return;
    }
    let down = m.spawnflags & DOOR_TOGGLE != 0 && matches!(m.state, State::Up | State::Top);
    for part in team_of(world, i) {
        if down {
            door_go_down(world, part);
        } else {
            door_go_up(world, part);
        }
    }
}

fn door_go_up(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    match m.state {
        State::Up => return,
        State::Top => {
            if m.wait_s >= 0.0 {
                m.think = Some((time + seconds_to_ms(m.wait_s), MoverThink::DoorGoDown));
            }
            return;
        }
        _ => {}
    }
    m.state = State::Up;
    let (rotating, entity, end_origin, end_angles) = (
        m.kind == MoverKind::DoorRotating,
        m.entity,
        m.end_origin,
        m.end_angles,
    );
    if rotating {
        angle_calc(world, i, end_angles, EndFunc::DoorHitTop);
    } else {
        move_calc(world, i, end_origin, EndFunc::DoorHitTop);
    }
    logic::use_targets(world, entity);
}

fn door_go_down(world: &mut World, i: usize) {
    let m = mv(world, i);
    if m.max_health != 0 {
        m.health = m.max_health;
    }
    m.state = State::Down;
    let (rotating, start_origin, start_angles) = (
        m.kind == MoverKind::DoorRotating,
        m.start_origin,
        m.start_angles,
    );
    if rotating {
        angle_calc(world, i, start_angles, EndFunc::DoorHitBottom);
    } else {
        move_calc(world, i, start_origin, EndFunc::DoorHitBottom);
    }
}

fn plat_go_down(world: &mut World, i: usize) {
    let m = mv(world, i);
    m.state = State::Down;
    let dest = m.end_origin;
    move_calc(world, i, dest, EndFunc::PlatHitBottom);
}

fn plat_go_up(world: &mut World, i: usize) {
    let m = mv(world, i);
    m.state = State::Up;
    let dest = m.start_origin;
    move_calc(world, i, dest, EndFunc::PlatHitTop);
}

fn button_fire(world: &mut World, i: usize) {
    let m = mv(world, i);
    if matches!(m.state, State::Up | State::Top) {
        return;
    }
    m.state = State::Up;
    let dest = m.end_origin;
    move_calc(world, i, dest, EndFunc::ButtonWait);
}

fn rotating_use(world: &mut World, i: usize) {
    let time = now(world);
    let m = mv(world, i);
    match m.state {
        State::Accel | State::FullSpeed => {
            if m.decel == 0.0 {
                m.avelocity = [0.0; 3];
                m.current_speed = 0.0;
                m.state = State::Stopped;
                m.think = None;
            } else {
                m.state = State::Decel;
                m.think = Some((time + 1, MoverThink::RotateDecel));
            }
        }
        _ => {
            if m.accel == 0.0 {
                m.avelocity = scale(m.movedir, m.speed);
                m.state = State::FullSpeed;
            } else {
                m.state = State::Accel;
                m.think = Some((time + 1, MoverThink::RotateAccel));
            }
        }
    }
}

fn rotate_think(world: &mut World, i: usize, accelerating: bool) {
    let time = now(world);
    let m = mv(world, i);
    if accelerating {
        if m.current_speed >= m.speed {
            m.current_speed = m.speed;
            m.avelocity = scale(m.movedir, m.speed);
            m.state = State::FullSpeed;
            return;
        }
        m.current_speed += m.accel;
        m.think = Some((time + 1, MoverThink::RotateAccel));
    } else {
        if m.current_speed <= 0.0 {
            m.current_speed = 0.0;
            m.avelocity = [0.0; 3];
            m.state = State::Stopped;
            return;
        }
        m.current_speed -= m.decel;
        m.think = Some((time + 1, MoverThink::RotateDecel));
    }
    m.avelocity = scale(m.movedir, m.current_speed);
}

/// A touch of a door or plat trigger volume by the living player.
pub(super) fn trigger_touched(world: &mut World, owner: usize, plat: bool) {
    let time = now(world);
    if plat {
        match mv(world, owner).state {
            State::Bottom => plat_go_up(world, owner),
            State::Top => mv(world, owner).think = Some((time + 1000, MoverThink::PlatGoDown)),
            _ => {}
        }
        return;
    }
    let m = mv(world, owner);
    if time < m.trigger_stamp + DOOR_TRIGGER_RETRIGGER_MS {
        return;
    }
    m.trigger_stamp = time;
    door_use(world, owner);
}

/// `button_touch`: the player walked into a solid button.
pub(super) fn touched_by_player(world: &mut World, lump: usize, time_ms: i64) {
    let Some(i) = world
        .resource::<Movers>()
        .list
        .iter()
        .position(|m| m.lump == lump)
    else {
        return;
    };
    if mv(world, i).touchable {
        world.resource_mut::<Level>().time_ms = time_ms;
        button_fire(world, i);
    }
}

/// `door_killed` / `button_killed`: damage dealt to a shootable mover.
pub(super) fn shoot(world: &mut World, lump: usize, damage: i32, time_ms: i64) -> bool {
    let Some(i) = world
        .resource::<Movers>()
        .list
        .iter()
        .position(|m| m.lump == lump)
    else {
        return false;
    };
    let m = mv(world, i);
    if m.max_health == 0 {
        return false;
    }
    m.health -= damage;
    if m.health > 0 {
        return true;
    }
    m.health = m.max_health;
    world.resource_mut::<Level>().time_ms = time_ms;
    use_mover(world, i);
    true
}

// ------------------------------------------------------------------ physics

/// Runs the movers for one tick: thinks, then the pusher of each team.
pub(super) fn step(
    world: &mut World,
    models: &dyn BrushModels,
    static_world: &dyn PmoveWorld,
    time_ms: i64,
    player: Option<PushedPlayer<'_>>,
) {
    world.resource_mut::<Level>().time_ms = time_ms;
    let mut ctx = Ctx {
        models,
        world: static_world,
        player,
    };
    let captains: Vec<usize> = world
        .resource::<Movers>()
        .list
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.follower)
        .map(|(i, _)| i)
        .collect();
    for captain in captains {
        world.resource_mut::<Movers>().current = Some(captain);
        let team = team_of(world, captain);
        for &part in &team {
            run_think(world, part);
        }
        physics_pusher(world, &mut ctx, &team, time_ms);
        world.resource_mut::<Movers>().current = None;
    }
}

fn physics_pusher(world: &mut World, ctx: &mut Ctx, team: &[usize], time_ms: i64) {
    let mut moved: Vec<(usize, Vec3, Vec3)> = Vec::new();
    let mut blocked = None;
    for &part in team {
        let m = mv(world, part);
        if is_zero(m.velocity) && is_zero(m.avelocity) {
            continue;
        }
        let amove = scale(m.avelocity, FRAME_S);
        let delta = match &m.linear {
            Some(linear) => sub(linear_position(linear, time_ms), m.origin),
            None => scale(m.velocity, FRAME_S),
        };
        let ok = if m.pushes {
            push(world, ctx, part, delta, amove)
        } else {
            let m = mv(world, part);
            m.origin = add(m.origin, delta);
            m.angles = add(m.angles, amove);
            true
        };
        if !ok {
            blocked = Some(part);
            break;
        }
        moved.push((part, delta, amove));
    }
    let Some(obstacle_of) = blocked else {
        return;
    };
    for (part, delta, amove) in moved {
        let m = mv(world, part);
        m.origin = sub(m.origin, delta);
        m.angles = sub(m.angles, amove);
    }
    for &part in team {
        if let Some((due, _)) = &mut mv(world, part).think {
            *due += FRAME_MS;
        }
    }
    blocked_by_player(world, obstacle_of);
}

/// `moveinfo.blocked` with the player as the obstacle.
fn blocked_by_player(world: &mut World, i: usize) {
    let m = mv(world, i);
    let (lump, dmg, kind, state, flags, wait) =
        (m.lump, m.dmg, m.kind, m.state, m.spawnflags, m.wait_s);
    if kind == MoverKind::Button {
        return;
    }
    world
        .resource_mut::<super::components::Outbox>()
        .events
        .push(TriggerEvent::Crush {
            entity: lump,
            damage: dmg,
        });
    match kind {
        MoverKind::Door | MoverKind::DoorRotating => {
            if flags & DOOR_CRUSHER != 0 || wait < 0.0 {
                return;
            }
            let leader = mv(world, i).leader;
            for part in team_of(world, leader) {
                if state == State::Down {
                    door_go_up(world, part);
                } else {
                    door_go_down(world, part);
                }
            }
        }
        MoverKind::Plat => match state {
            State::Up => plat_go_down(world, i),
            State::Down => plat_go_up(world, i),
            _ => {}
        },
        _ => {}
    }
}

fn blocked_at(world: &World, ctx: &Ctx, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
    let all = MoverWorld::new(ctx.world, ctx.models, poses(world));
    let trace = all.trace(origin, mins, maxs, origin, MASK_PLAYERSOLID);
    trace.start_solid || trace.all_solid
}

/// `SV_Push` for one pusher; false when the player blocks it.
fn push(world: &mut World, ctx: &mut Ctx, i: usize, delta: Vec3, amove: Vec3) -> bool {
    let m = mv(world, i);
    let (old_origin, old_angles) = (m.origin, m.angles);
    m.origin = add(m.origin, delta);
    m.angles = add(m.angles, amove);
    let (lump, pusher_origin, bounds) = (
        m.lump,
        m.origin,
        abs_bounds(m.origin, m.angles, m.mins, m.maxs),
    );

    let Some((origin, mins, maxs, ground)) = ctx
        .player
        .as_ref()
        .filter(|p| p.state.pm_type <= PmType::Normal)
        .map(|p| (p.state.origin, p.state.mins, p.state.maxs, p.ground_entity))
    else {
        return true;
    };
    let riding = ground == Some(lump as i32);
    if !riding {
        let overlaps = (0..3).all(|axis| {
            origin[axis] + mins[axis] < bounds.1[axis] && origin[axis] + maxs[axis] > bounds.0[axis]
        });
        if !overlaps || !blocked_at(world, ctx, origin, mins, maxs) {
            return true;
        }
    }

    let mut pushed = add(origin, delta);
    if !is_zero(amove) {
        let relative = sub(pushed, pusher_origin);
        let rows = axis_rows(scale(amove, -1.0));
        let rotated = [
            dot(rows[0], relative),
            dot(rows[1], relative),
            dot(rows[2], relative),
        ];
        pushed = add(pushed, sub(rotated, relative));
    }
    if !blocked_at(world, ctx, pushed, mins, maxs) {
        if let Some(player) = ctx.player.as_mut() {
            player.state.origin = pushed;
        }
        return true;
    }
    // Staying put is fine when the old position is still free.
    if !blocked_at(world, ctx, origin, mins, maxs) {
        return true;
    }
    let m = mv(world, i);
    m.origin = old_origin;
    m.angles = old_angles;
    false
}
