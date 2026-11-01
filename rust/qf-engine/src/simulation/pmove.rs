#![allow(clippy::needless_range_loop)] // index loops mirror the legacy per-axis C code
//! Port of the legacy Warsow player movement (`source/common/facilities/gs_pmove.cpp`).
//!
//! The implementation is independent of rendering, windowing and the BSP
//! format: it moves a [`PmoveState`] through any [`PmoveWorld`]. The same code
//! can therefore run for local play, server authority and client prediction.
//!
//! Arithmetic deliberately mirrors the legacy `float` code, including its few
//! `double` promotions (`M_PI`, `0.001`, `SLIDEMOVE_PLANEINTERACT_EPSILON`),
//! because the ordering of branches and the exact event/flag transitions are
//! compared against recorded legacy traces. Trigger touching and entity
//! interaction are game-side concerns: they plug in through [`PmoveTouch`].

use super::directions::dir_to_byte;

pub type Vec3 = [f32; 3];

pub const GRAVITY: i32 = 850;
pub const BASE_GRAVITY: i32 = 800;
pub const GRAVITY_COMPENSATE: f32 = GRAVITY as f32 / BASE_GRAVITY as f32;
pub const STEP_SIZE: f32 = 18.0;
pub const CROUCH_TIME: i32 = 100;
pub const ZOOM_TIME: i32 = 60;
pub const DEFAULT_PLAYER_SPEED: f32 = 320.0;
pub const DEFAULT_JUMP_SPEED: f32 = 280.0;
pub const DEFAULT_DASH_SPEED: f32 = 450.0;
pub const MAX_TOUCH: usize = 32;

pub const PLAYER_STAND_MINS: Vec3 = [-16.0, -16.0, -24.0];
pub const PLAYER_STAND_MAXS: Vec3 = [16.0, 16.0, 40.0];
pub const PLAYER_STAND_VIEWHEIGHT: i32 = 30;
pub const PLAYER_CROUCH_MINS: Vec3 = [-16.0, -16.0, -24.0];
pub const PLAYER_CROUCH_MAXS: Vec3 = [16.0, 16.0, 16.0];
pub const PLAYER_CROUCH_VIEWHEIGHT: i32 = 12;
pub const PLAYER_GIB_MINS: Vec3 = [-16.0, -16.0, 0.0];
pub const PLAYER_GIB_MAXS: Vec3 = [16.0, 16.0, 16.0];
pub const PLAYER_GIB_VIEWHEIGHT: i32 = 8;

pub const CONTENTS_SOLID: u32 = 1;
pub const CONTENTS_LAVA: u32 = 8;
pub const CONTENTS_SLIME: u32 = 16;
pub const CONTENTS_WATER: u32 = 32;
pub const CONTENTS_PLAYERCLIP: u32 = 0x10000;
pub const CONTENTS_BODY: u32 = 0x200_0000;
pub const MASK_PLAYERSOLID: u32 = CONTENTS_SOLID | CONTENTS_PLAYERCLIP | CONTENTS_BODY;
pub const MASK_DEADSOLID: u32 = CONTENTS_SOLID | CONTENTS_PLAYERCLIP;
pub const MASK_WATER: u32 = CONTENTS_WATER | CONTENTS_LAVA | CONTENTS_SLIME;

pub const SURF_NODAMAGE: u32 = 0x1;
pub const SURF_SLICK: u32 = 0x2;
pub const SURF_SKY: u32 = 0x4;
pub const SURF_LADDER: u32 = 0x8;
pub const SURF_NOWALLJUMP: u32 = 0x8_0000;

pub const BUTTON_WALK: u8 = 2;
pub const BUTTON_SPECIAL: u8 = 4;
pub const BUTTON_ZOOM: u8 = 16;

pub const PMF_WALLJUMPCOUNT: u32 = 1 << 0;
pub const PMF_JUMP_HELD: u32 = 1 << 1;
pub const PMF_ON_GROUND: u32 = 1 << 2;
pub const PMF_TIME_WATERJUMP: u32 = 1 << 3;
pub const PMF_TIME_LAND: u32 = 1 << 4;
pub const PMF_TIME_TELEPORT: u32 = 1 << 5;
pub const PMF_NO_PREDICTION: u32 = 1 << 6;
pub const PMF_DASHING: u32 = 1 << 7;
pub const PMF_SPECIAL_HELD: u32 = 1 << 8;
pub const PMF_WALLJUMPING: u32 = 1 << 9;
pub const PMF_DOUBLEJUMPED: u32 = 1 << 10;
pub const PMF_JUMPPAD_TIME: u32 = 1 << 11;
pub const PMF_CROUCH_SLIDING: u32 = 1 << 12;

pub const PMFEAT_CROUCH: u32 = 1 << 0;
pub const PMFEAT_WALK: u32 = 1 << 1;
pub const PMFEAT_JUMP: u32 = 1 << 2;
pub const PMFEAT_DASH: u32 = 1 << 3;
pub const PMFEAT_WALLJUMP: u32 = 1 << 4;
pub const PMFEAT_FWDBUNNY: u32 = 1 << 5;
pub const PMFEAT_AIRCONTROL: u32 = 1 << 6;
pub const PMFEAT_ZOOM: u32 = 1 << 7;
pub const PMFEAT_GHOSTMOVE: u32 = 1 << 8;
pub const PMFEAT_CONTINOUSJUMP: u32 = 1 << 9;
pub const PMFEAT_CROUCHSLIDING: u32 = 1 << 14;
pub const PMFEAT_ALL: u32 = 0xFFFF;
pub const PMFEAT_DEFAULT: u32 = PMFEAT_ALL & !(PMFEAT_GHOSTMOVE | PMFEAT_CROUCHSLIDING);

const SPEEDKEY: f32 = 500.0;
const PM_DASHJUMP_TIMEDELAY: i32 = 1000;
const PM_WALLJUMP_TIMEDELAY: i32 = 1300;
const PM_WALLJUMP_FAILED_TIMEDELAY: i32 = 700;
const PM_SPECIAL_CROUCH_INHIBIT: i32 = 400;
const PM_AIRCONTROL_BOUNCE_DELAY: i32 = 200;
const PM_OVERBOUNCE: f32 = 1.01;
const PM_CROUCHSLIDE: i32 = 1500;
const PM_CROUCHSLIDE_FADE: i32 = 500;
const PM_CROUCHSLIDE_TIMEDELAY: i32 = 700;
const PM_CROUCHSLIDE_CONTROL: f32 = 3.0;
const PM_FORWARD_ACCEL_TIMEDELAY: i32 = 0;

const DEFAULT_WALKSPEED: f32 = 160.0;
const DEFAULT_CROUCHEDSPEED: f32 = 100.0;
const DEFAULT_LADDERSPEED: f32 = 250.0;

const PM_FRICTION: f32 = 8.0;
const PM_WATERFRICTION: f32 = 1.0;
const PM_WATERACCELERATE: f32 = 10.0;
const PM_ACCELERATE: f32 = 12.0;
const PM_DECELERATE: f32 = 12.0;
const PM_AIRACCELERATE: f32 = 1.0;
const PM_AIRDECELERATE: f32 = 2.0;
const PM_AIRCONTROL: f32 = 150.0;
const PM_STRAFEBUNNYACCEL: f32 = 70.0;
const PM_WISHSPEED: f32 = 30.0;

const PM_DASHUPSPEED: f32 = 174.0 * GRAVITY_COMPENSATE;
const PM_WJUPSPEED: f32 = 330.0 * GRAVITY_COMPENSATE;
const PM_FAILEDWJUPSPEED: f32 = 50.0 * GRAVITY_COMPENSATE;
const PM_WJBOUNCEFACTOR: f32 = 0.3;
const PM_FAILEDWJBOUNCEFACTOR: f32 = 0.1;

const SLIDEMOVE_PLANEINTERACT_EPSILON: f64 = 0.05;
const SLIDEMOVEFLAG_WALL_BLOCKED: u32 = 8;
const SLIDEMOVEFLAG_TRAPPED: u32 = 4;
const SLIDEMOVEFLAG_BLOCKED: u32 = 2;
const MAX_CLIP_PLANES: usize = 5;

const FALL_DAMAGE_MIN_DELTA: f32 = 675.0;
const FALL_STEP_MIN_DELTA: f32 = 400.0;
const MAX_FALLING_DAMAGE: f32 = 15.0;
const FALL_DAMAGE_SCALE: f32 = 1.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub enum PmType {
    Normal,
    /// Free flight without collision: the legacy noclip/spectator movement.
    Spectator,
    Gib,
    Freeze,
    Chasecam,
}

/// Events emitted by prediction. Numeric values match the legacy event ids.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PmEvent {
    Dash = 9,
    WallJump = 10,
    WallJumpFailed = 11,
    DoubleJump = 12,
    Jump = 13,
    JumpPad = 14,
    Fall = 15,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PredictedEvent {
    pub event: PmEvent,
    pub parameter: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PmTrace {
    pub fraction: f32,
    pub end_position: Vec3,
    pub plane_normal: Vec3,
    pub surface_flags: u32,
    pub contents: u32,
    pub entity: i32,
    pub start_solid: bool,
    pub all_solid: bool,
}

impl PmTrace {
    pub fn miss(end: Vec3) -> Self {
        Self {
            fraction: 1.0,
            end_position: end,
            plane_normal: [0.0; 3],
            surface_flags: 0,
            contents: 0,
            entity: -1,
            start_solid: false,
            all_solid: false,
        }
    }
}

/// Collision and contents queries needed by movement.
pub trait PmoveWorld {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, contents_mask: u32) -> PmTrace;
    fn point_contents(&self, point: Vec3) -> u32;
    fn is_player_entity(&self, _entity: i32) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PmoveStats {
    pub features: u32,
    pub no_user_control: i32,
    pub knockback: i32,
    pub crouch_time: i32,
    pub zoom_time: i32,
    pub dash_time: i32,
    pub wall_jump_time: i32,
    pub no_auto_attack: i32,
    pub stun: i32,
    /// Negative selects the default.
    pub max_speed: i32,
    pub jump_speed: i32,
    pub dash_speed: i32,
    pub forward_time: i32,
    pub crouch_slide_time: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PmoveState {
    pub pm_type: PmType,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub flags: u32,
    pub pm_time: i32,
    pub gravity: i32,
    pub delta_angles: [i16; 3],
    pub stats: PmoveStats,
    pub view_angles: Vec3,
    pub view_height: i32,
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl PmoveState {
    pub fn new(origin: Vec3) -> Self {
        Self {
            pm_type: PmType::Normal,
            origin,
            velocity: [0.0; 3],
            flags: 0,
            pm_time: 0,
            gravity: GRAVITY,
            delta_angles: [0; 3],
            stats: PmoveStats {
                features: PMFEAT_DEFAULT,
                max_speed: DEFAULT_PLAYER_SPEED as i32,
                jump_speed: DEFAULT_JUMP_SPEED as i32,
                dash_speed: DEFAULT_DASH_SPEED as i32,
                ..PmoveStats::default()
            },
            view_angles: [0.0; 3],
            view_height: PLAYER_STAND_VIEWHEIGHT,
            mins: PLAYER_STAND_MINS,
            maxs: PLAYER_STAND_MAXS,
        }
    }

    pub fn on_ground(&self) -> bool {
        self.flags & PMF_ON_GROUND != 0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PmoveCommand {
    pub msec: u8,
    pub buttons: u8,
    pub forward_move: i8,
    pub side_move: i8,
    pub up_move: i8,
    /// Pitch, yaw and roll as 16-bit angles (65536 per turn).
    pub angles: [i16; 3],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PmoveRules {
    pub fall_damage: bool,
    pub match_paused: bool,
    pub snap_initially: bool,
    pub skip_collision: bool,
    pub skip_ladders: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PmoveResult {
    pub events: Vec<PredictedEvent>,
    pub touch_entities: Vec<i32>,
    /// Entity standing on, or `None` when airborne.
    pub ground_entity: Option<i32>,
    pub water_level: u8,
    pub water_type: u32,
    pub ladder: bool,
    pub step: f32,
}

pub fn angle_to_short(angle: f32) -> i32 {
    ((angle * 65536.0 / 360.0) as i32) & 65535
}

pub fn short_to_angle(value: i16) -> f32 {
    (f64::from(value) * (360.0 / 65536.0)) as f32
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dot2(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn ma(v: Vec3, scale: f32, dir: Vec3) -> Vec3 {
    [
        v[0] + scale * dir[0],
        v[1] + scale * dir[1],
        v[2] + scale * dir[2],
    ]
}

fn scale(v: Vec3, s: f32) -> Vec3 {
    [v[0] * s, v[1] * s, v[2] * s]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn length(v: Vec3) -> f32 {
    dot(v, v).sqrt()
}

fn normalize(v: &mut Vec3) -> f32 {
    let square = dot(*v, *v);
    let mut len = 0.0;
    if square > 0.0 {
        len = square.sqrt();
        let inverse = 1.0 / len;
        v[0] *= inverse;
        v[1] *= inverse;
        v[2] *= inverse;
    }
    len
}

fn normalize2d(v: &mut Vec3) -> f32 {
    let square = v[0] * v[0] + v[1] * v[1];
    if square != 0.0 {
        let len = (f64::from(square)).sqrt() as f32;
        let inverse = 1.0 / len;
        v[0] *= inverse;
        v[1] *= inverse;
        return len;
    }
    0.0
}

fn lerp(a: Vec3, fraction: f32, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] + fraction * (b[i] - a[i]))
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// `GS_ClipVelocity`.
fn clip_velocity(velocity: Vec3, normal: Vec3, overbounce: f32) -> Vec3 {
    let mut backoff = dot(velocity, normal);
    if backoff <= 0.0 {
        backoff *= overbounce;
    } else {
        backoff /= overbounce;
    }
    std::array::from_fn(|i| velocity[i] - normal[i] * backoff)
}

/// `AngleVectors`: returns forward, right and up. Positive pitch looks down.
pub fn angle_vectors(angles: Vec3) -> (Vec3, Vec3, Vec3) {
    let deg2rad = std::f32::consts::PI / 180.0;
    let (sin_yaw, cos_yaw) = (deg2rad * angles[1]).sin_cos();
    let (sin_pitch, cos_pitch) = (deg2rad * angles[0]).sin_cos();
    let (sin_roll, cos_roll) = if angles[2] != 0.0 {
        (deg2rad * angles[2]).sin_cos()
    } else {
        (0.0, 1.0)
    };
    let forward = [cos_pitch * cos_yaw, cos_pitch * sin_yaw, -sin_pitch];
    let t = sin_roll * sin_pitch;
    let right = [
        -t * cos_yaw + -cos_roll * -sin_yaw,
        -t * sin_yaw + -cos_roll * cos_yaw,
        -sin_roll * cos_pitch,
    ];
    let t = cos_roll * sin_pitch;
    let up = [
        t * cos_yaw + -sin_roll * -sin_yaw,
        t * sin_yaw + -sin_roll * cos_yaw,
        cos_roll * cos_pitch,
    ];
    (forward, right, up)
}

fn is_walkable(normal: Vec3) -> bool {
    f64::from(normal[2]) >= 0.7
}

/// Pose of the player before the current command, for swept trigger touching.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchContext {
    pub previous_origin: Vec3,
    pub previous_velocity: Vec3,
    /// Pitch/yaw/roll in the legacy convention (pitch positive downward).
    pub command_angles: [i16; 3],
}

/// Game-side trigger touching, called where the legacy `PMoveTouchTriggers`
/// hook runs: after the move completes and before delta angles are refreshed.
pub trait PmoveTouch {
    fn touch_triggers(
        &mut self,
        state: &mut PmoveState,
        context: &TouchContext,
        events: &mut Vec<PredictedEvent>,
    );
}

/// Advances one movement command (`Pmove`).
pub fn pmove<W: PmoveWorld>(
    world: &W,
    rules: &PmoveRules,
    state: &mut PmoveState,
    command: &PmoveCommand,
) -> PmoveResult {
    Mover::new(world, rules, state, *command, None).run()
}

/// `pmove` that also touches triggers and keeps delta angles in sync, as the
/// legacy game module does.
pub fn pmove_with_touch<W: PmoveWorld>(
    world: &W,
    rules: &PmoveRules,
    state: &mut PmoveState,
    command: &PmoveCommand,
    touch: &mut dyn PmoveTouch,
) -> PmoveResult {
    Mover::new(world, rules, state, *command, Some(touch)).run()
}

struct Mover<'a, W: PmoveWorld> {
    world: &'a W,
    rules: &'a PmoveRules,
    ps: &'a mut PmoveState,
    touch_hook: Option<&'a mut dyn PmoveTouch>,
    previous_velocity: Vec3,
    cmd: PmoveCommand,
    contents_mask: u32,
    events: Vec<PredictedEvent>,
    touch: Vec<i32>,
    ground_entity: i32,
    water_level: u8,
    water_type: u32,
    pm_ladder: bool,
    step: f32,

    origin: Vec3,
    velocity: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    flat_forward: Vec3,
    frame_time: f32,
    ground_surface_flags: u32,
    ground_normal: Vec3,
    previous_origin: Vec3,
    ladder: bool,
    forward_push: f32,
    side_push: f32,
    up_push: f32,
    max_player_speed: f32,
    max_walk_speed: f32,
    max_crouched_speed: f32,
    jump_player_speed: f32,
    dash_player_speed: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CalcPosition {
    WasGood,
    Corrected,
    Unsolved,
}

impl<'a, W: PmoveWorld> Mover<'a, W> {
    fn new(
        world: &'a W,
        rules: &'a PmoveRules,
        ps: &'a mut PmoveState,
        cmd: PmoveCommand,
        touch_hook: Option<&'a mut dyn PmoveTouch>,
    ) -> Self {
        Self {
            world,
            rules,
            ps,
            touch_hook,
            previous_velocity: [0.0; 3],
            cmd,
            contents_mask: 0,
            events: Vec::new(),
            touch: Vec::new(),
            ground_entity: -1,
            water_level: 0,
            water_type: 0,
            pm_ladder: false,
            step: 0.0,
            origin: [0.0; 3],
            velocity: [0.0; 3],
            forward: [0.0; 3],
            right: [0.0; 3],
            up: [0.0; 3],
            flat_forward: [0.0; 3],
            frame_time: 0.0,
            ground_surface_flags: 0,
            ground_normal: [0.0; 3],
            previous_origin: [0.0; 3],
            ladder: false,
            forward_push: 0.0,
            side_push: 0.0,
            up_push: 0.0,
            max_player_speed: 0.0,
            max_walk_speed: 0.0,
            max_crouched_speed: 0.0,
            jump_player_speed: 0.0,
            dash_player_speed: 0.0,
        }
    }

    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3) -> PmTrace {
        self.world.trace(start, mins, maxs, end, self.contents_mask)
    }

    fn emit(&mut self, event: PmEvent, parameter: i32) {
        self.events.push(PredictedEvent { event, parameter });
    }

    fn add_touch_entity(&mut self, entity: i32) {
        if self.touch.len() >= MAX_TOUCH || entity < 0 || self.touch.contains(&entity) {
            return;
        }
        self.touch.push(entity);
    }

    fn feature(&self, feature: u32) -> bool {
        self.ps.stats.features & feature != 0
    }

    fn flag(&self, flag: u32) -> bool {
        self.ps.flags & flag != 0
    }

    fn player_touch_wall(&self, directions: usize, max_z_normal: f32) -> Vec3 {
        let zero = [0.0; 3];
        let (pm_mins, pm_maxs) = (self.ps.mins, self.ps.maxs);

        // Cheap early-out that must over-approximate every individual check below.
        let mut mins = [pm_mins[0] - pm_maxs[0], pm_mins[1] - pm_maxs[0], 0.0];
        let mut maxs = [pm_maxs[0] + pm_maxs[0], pm_maxs[1] + pm_maxs[0], 0.0];
        if self.velocity[0] > 0.0 {
            maxs[0] += self.velocity[0] * 0.015;
        } else {
            mins[0] += self.velocity[0] * 0.015;
        }
        if self.velocity[1] > 0.0 {
            maxs[1] += self.velocity[1] * 0.015;
        } else {
            mins[1] += self.velocity[1] * 0.015;
        }
        let trace = self.trace(self.origin, mins, maxs, self.origin);
        if !trace.all_solid && trace.fraction == 1.0 {
            return zero;
        }

        let pi = std::f64::consts::PI;
        let mut r: f32 = if self.side_push > 0.0 {
            (-pi / 2.0) as f32
        } else if self.side_push < 0.0 {
            (pi / 2.0) as f32
        } else if self.forward_push > 0.0 {
            0.0
        } else {
            pi as f32
        };
        let alternate = self.side_push == 0.0 || self.forward_push == 0.0;
        let mut d: f32 = 0.0;
        let yaw = f64::from(self.ps.view_angles[1]);

        for i in 0..directions {
            if i != 0 {
                if alternate {
                    r = (f64::from(r) + pi) as f32;
                }
                if (!alternate && i % 2 == 0) || (alternate && i % 4 == 0) {
                    r -= 2.0 * d;
                } else if !alternate || (alternate && i % 4 == 2) {
                    r += d;
                    d = (f64::from(d) + std::f64::consts::TAU / directions as f64) as f32;
                    r += d;
                }
            }

            let angle = (yaw * pi) / 180.0 + f64::from(r);
            let dx = angle.cos() as f32;
            let dy = angle.sin() as f32;

            let mut m = if dx == 0.0 {
                pm_maxs[1]
            } else if dy == 0.0 {
                pm_maxs[0]
            } else if (dx / pm_maxs[0]).abs() > (dy / pm_maxs[1]).abs() {
                (pm_maxs[0] / dx).abs()
            } else {
                (pm_maxs[1] / dy).abs()
            };
            m += pm_maxs[0];

            let dir = [
                self.origin[0] + dx * m + self.velocity[0] * 0.015,
                self.origin[1] + dy * m + self.velocity[1] * 0.015,
                self.origin[2],
            ];
            let trace = self.trace(self.origin, zero, zero, dir);
            if trace.all_solid {
                return zero;
            }
            if trace.fraction == 1.0 {
                continue;
            }
            if trace.surface_flags & (SURF_SKY | SURF_NOWALLJUMP) != 0 {
                continue;
            }
            if trace.entity > 0 && self.world.is_player_entity(trace.entity) {
                continue;
            }
            if trace.fraction > 0.0 && trace.plane_normal[2].abs() < max_z_normal {
                return trace.plane_normal;
            }
        }
        zero
    }

    fn slide_move(&mut self) -> u32 {
        let old_velocity = self.velocity;
        let mut last_valid_origin = self.origin;
        let mut remaining_time = self.frame_time;
        let mut blocked = 0;

        if self.ground_entity != -1 && self.ground_normal[2] == 1.0 && self.velocity[2] < 0.0 {
            self.velocity[2] = 0.0;
        }

        if self.rules.skip_collision {
            self.origin = ma(self.origin, remaining_time, self.velocity);
            return blocked;
        }

        let mut planes = [[0.0f32; 3]; MAX_CLIP_PLANES];
        let mut num_planes = 0;
        let (mins, maxs) = (self.ps.mins, self.ps.maxs);

        for _ in 0..4 {
            let end = ma(self.origin, remaining_time, self.velocity);
            let trace = self.trace(self.origin, mins, maxs, end);
            if trace.all_solid {
                self.origin = last_valid_origin;
                return SLIDEMOVEFLAG_TRAPPED;
            }
            if trace.fraction > 0.0 {
                self.origin = trace.end_position;
                last_valid_origin = trace.end_position;
            }
            if trace.fraction == 1.0 {
                break;
            }
            self.add_touch_entity(trace.entity);

            blocked |= SLIDEMOVEFLAG_BLOCKED;
            if f64::from(trace.plane_normal[2]) < SLIDEMOVE_PLANEINTERACT_EPSILON {
                blocked |= SLIDEMOVEFLAG_WALL_BLOCKED;
            }
            remaining_time -= trace.fraction * remaining_time;

            let mut repeated = false;
            for plane in &planes[..num_planes] {
                if f64::from(dot(trace.plane_normal, *plane))
                    > 1.0 - SLIDEMOVE_PLANEINTERACT_EPSILON
                {
                    self.velocity = [
                        trace.plane_normal[0] + self.velocity[0],
                        trace.plane_normal[1] + self.velocity[1],
                        trace.plane_normal[2] + self.velocity[2],
                    ];
                    repeated = true;
                    break;
                }
            }
            if repeated {
                continue;
            }
            if num_planes >= MAX_CLIP_PLANES {
                self.velocity = [0.0; 3];
                return SLIDEMOVEFLAG_TRAPPED;
            }
            planes[num_planes] = trace.plane_normal;
            num_planes += 1;

            let touches =
                |v: Vec3, plane: Vec3| f64::from(dot(v, plane)) >= SLIDEMOVE_PLANEINTERACT_EPSILON;
            for i in 0..num_planes {
                if touches(self.velocity, planes[i]) {
                    continue;
                }
                self.velocity = clip_velocity(self.velocity, planes[i], PM_OVERBOUNCE);
                for j in 0..num_planes {
                    if j == i || touches(self.velocity, planes[j]) {
                        continue;
                    }
                    self.velocity = clip_velocity(self.velocity, planes[j], PM_OVERBOUNCE);
                    if touches(self.velocity, planes[i]) {
                        continue;
                    }
                    let mut dir = cross(planes[i], planes[j]);
                    normalize(&mut dir);
                    let value = dot(dir, self.velocity);
                    self.velocity = scale(dir, value);
                    for k in 0..num_planes {
                        if j == k || i == k || touches(self.velocity, planes[k]) {
                            continue;
                        }
                        self.velocity = [0.0; 3];
                        break;
                    }
                }
            }
        }

        if self.ps.pm_time != 0 {
            self.velocity = old_velocity;
        }
        blocked
    }

    fn step_slide_move(&mut self) {
        let start_o = self.origin;
        let start_v = self.velocity;
        let blocked = self.slide_move();
        if self.rules.skip_collision {
            return;
        }

        let down_o = self.origin;
        let down_v = self.velocity;
        let (mins, maxs) = (self.ps.mins, self.ps.maxs);

        let mut up = start_o;
        up[2] += STEP_SIZE;
        if self.trace(up, mins, maxs, up).all_solid {
            return;
        }

        self.origin = up;
        self.velocity = start_v;
        self.slide_move();

        let mut down = self.origin;
        down[2] -= STEP_SIZE;
        let trace = self.trace(self.origin, mins, maxs, down);
        if !trace.all_solid {
            self.origin = trace.end_position;
        }
        let up = self.origin;

        let down_dist = (down_o[0] - start_o[0]) * (down_o[0] - start_o[0])
            + (down_o[1] - start_o[1]) * (down_o[1] - start_o[1]);
        let up_dist = (up[0] - start_o[0]) * (up[0] - start_o[0])
            + (up[1] - start_o[1]) * (up[1] - start_o[1]);

        if down_dist >= up_dist
            || trace.all_solid
            || (trace.fraction != 1.0 && !is_walkable(trace.plane_normal))
        {
            self.origin = down_o;
            self.velocity = down_v;
            return;
        }

        // Only a vertical step (or the exit of a ramp) reports a step height.
        if blocked & SLIDEMOVEFLAG_WALL_BLOCKED != 0
            || f64::from(trace.plane_normal[2]) == 1.0 - SLIDEMOVE_PLANEINTERACT_EPSILON
        {
            self.step = self.origin[2] - self.previous_origin[2];
        }

        let hspeed = (start_v[0] * start_v[0] + start_v[1] * start_v[1]).sqrt();
        if hspeed != 0.0 && is_walkable(trace.plane_normal) {
            if f64::from(trace.plane_normal[2]) >= 1.0 - SLIDEMOVE_PLANEINTERACT_EPSILON {
                self.velocity = start_v;
            } else {
                normalize2d(&mut self.velocity);
                self.velocity[0] *= hspeed;
                self.velocity[1] *= hspeed;
            }
        }

        // Walking along a plane keeps the vertical velocity of the direct slide.
        self.velocity[2] = down_v[2];
    }

    fn friction(&mut self) {
        let speed = dot(self.velocity, self.velocity);
        if speed < 1.0 {
            self.velocity[0] = 0.0;
            self.velocity[1] = 0.0;
            return;
        }
        let speed = speed.sqrt();
        let mut drop = 0.0;

        if ((self.ground_entity != -1
            && self.ground_surface_flags & SURF_SLICK == 0
            && self.water_level < 2)
            || self.ladder)
            && self.ps.stats.knockback <= 0
        {
            let mut friction = PM_FRICTION;
            let control = if speed < PM_DECELERATE {
                PM_DECELERATE
            } else {
                speed
            };
            if self.flag(PMF_CROUCH_SLIDING) {
                if self.ps.stats.crouch_slide_time < PM_CROUCHSLIDE_FADE {
                    let ratio = self.ps.stats.crouch_slide_time as f32 / PM_CROUCHSLIDE_FADE as f32;
                    friction = (f64::from(friction) * (1.0 - f64::from(ratio).sqrt())) as f32;
                } else {
                    friction = 0.0;
                }
            }
            drop += control * friction * self.frame_time;
        }

        if self.water_level >= 2 && !self.ladder {
            drop += speed * PM_WATERFRICTION * f32::from(self.water_level) * self.frame_time;
        }

        let new_speed = speed - drop;
        if new_speed <= 0.0 {
            self.velocity = [0.0; 3];
        } else {
            self.velocity = scale(self.velocity, new_speed / speed);
        }
    }

    fn accelerate(&mut self, wish_dir: Vec3, wish_speed: f32, accel: f32) {
        let real_speed = length(self.velocity);
        let current_speed = dot(self.velocity, wish_dir);
        let add_speed = wish_speed - current_speed;
        if add_speed <= 0.0 {
            return;
        }

        let mut accel_speed = accel * self.frame_time * wish_speed;
        if accel_speed > add_speed {
            accel_speed = add_speed;
        }

        let crouch_slide = self.flag(PMF_CROUCH_SLIDING)
            && self.ground_entity != -1
            && self.ground_surface_flags & SURF_SLICK == 0;
        if crouch_slide {
            accel_speed *= PM_CROUCHSLIDE_CONTROL;
        }

        self.velocity = ma(self.velocity, accel_speed, wish_dir);

        if crouch_slide {
            let new_speed = length(self.velocity);
            if new_speed > wish_speed && new_speed != 0.0 {
                self.velocity = scale(self.velocity, wish_speed.max(real_speed) / new_speed);
            }
        }
    }

    fn air_accelerate(&mut self, wish_dir: Vec3, wish_speed: f32) {
        const AIR_FORWARD_ACCEL: f32 = 1.00001;
        const BUNNY_ACCEL: f32 = 0.1593;
        const BUNNY_TOP_SPEED: f32 = 925.0;
        const TURN_ACCEL: f32 = 4.0;
        const BACK_TO_SIDE_RATIO: f32 = 0.8;

        if wish_speed == 0.0 {
            return;
        }
        let mut wish_speed = wish_speed;

        let mut cur_vel = self.velocity;
        cur_vel[2] = 0.0;
        let cur_speed = length(cur_vel);

        if wish_speed > cur_speed * 1.01 {
            let accel_speed =
                cur_speed + AIR_FORWARD_ACCEL * self.max_player_speed * self.frame_time;
            if accel_speed < wish_speed {
                wish_speed = accel_speed;
            }
        } else {
            let mut f = (BUNNY_TOP_SPEED - cur_speed) / (BUNNY_TOP_SPEED - self.max_player_speed);
            if f < 0.0 {
                f = 0.0;
            }
            wish_speed = cur_speed.max(self.max_player_speed)
                + BUNNY_ACCEL * f * self.max_player_speed * self.frame_time;
        }
        let wish_vel = scale(wish_dir, wish_speed);
        let mut accel_dir = sub(wish_vel, cur_vel);
        let add_speed = normalize(&mut accel_dir);

        let mut accel_speed = TURN_ACCEL * self.max_player_speed * self.frame_time;
        if accel_speed > add_speed {
            accel_speed = add_speed;
        }

        if BACK_TO_SIDE_RATIO < 1.0 {
            let mut cur_dir = cur_vel;
            normalize(&mut cur_dir);
            let d = dot(accel_dir, cur_dir);
            if d < 0.0 {
                accel_dir = ma(accel_dir, -(1.0 - BACK_TO_SIDE_RATIO) * d, cur_dir);
            }
        }

        self.velocity = ma(self.velocity, accel_speed, accel_dir);
    }

    fn air_control(&mut self, wish_dir: Vec3, wish_speed: f32) {
        if self.side_push != 0.0 || wish_speed == 0.0 {
            return;
        }
        let z_speed = self.velocity[2];
        self.velocity[2] = 0.0;
        let speed = normalize(&mut self.velocity);

        let d = dot(self.velocity, wish_dir);
        let k = 32.0 * PM_AIRCONTROL * d * d * self.frame_time;

        if d > 0.0 {
            // Direction cannot change while slowing down.
            for i in 0..2 {
                self.velocity[i] = self.velocity[i] * speed + wish_dir[i] * k;
            }
            normalize(&mut self.velocity);
        }

        for i in 0..2 {
            self.velocity[i] *= speed;
        }
        self.velocity[2] = z_speed;
    }

    fn add_currents(&self, wish_vel: &mut Vec3) {
        if self.ladder && self.velocity[2].abs() <= DEFAULT_LADDERSPEED {
            if self.ps.view_angles[0] <= -15.0 && self.forward_push > 0.0 {
                wish_vel[2] = DEFAULT_LADDERSPEED;
            } else if self.ps.view_angles[0] >= 15.0 && self.forward_push > 0.0 {
                wish_vel[2] = -DEFAULT_LADDERSPEED;
            } else if self.up_push > 0.0 {
                wish_vel[2] = DEFAULT_LADDERSPEED;
            } else if self.up_push < 0.0 {
                wish_vel[2] = -DEFAULT_LADDERSPEED;
            } else {
                wish_vel[2] = 0.0;
            }
            wish_vel[0] = wish_vel[0].clamp(-25.0, 25.0);
            wish_vel[1] = wish_vel[1].clamp(-25.0, 25.0);
        }
    }

    fn water_move(&mut self) {
        let mut wish_vel: Vec3 = std::array::from_fn(|i| {
            self.forward[i] * self.forward_push + self.right[i] * self.side_push
        });
        if self.forward_push == 0.0 && self.side_push == 0.0 && self.up_push == 0.0 {
            wish_vel[2] -= 60.0;
        } else {
            wish_vel[2] += self.up_push;
        }

        self.add_currents(&mut wish_vel);

        let mut wish_dir = wish_vel;
        let mut wish_speed = normalize(&mut wish_dir);
        if wish_speed > self.max_player_speed {
            wish_speed = self.max_player_speed;
        }
        wish_speed *= 0.5;

        self.accelerate(wish_dir, wish_speed, PM_WATERACCELERATE);
        self.step_slide_move();
    }

    fn add_gravity(&mut self) {
        self.velocity[2] -= self.ps.gravity as f32 * self.frame_time;
    }

    fn move_walk(&mut self) {
        let fmove = self.forward_push;
        let smove = self.side_push;

        let mut wish_vel = [
            self.forward[0] * fmove + self.right[0] * smove,
            self.forward[1] * fmove + self.right[1] * smove,
            0.0,
        ];
        self.add_currents(&mut wish_vel);

        let mut wish_dir = wish_vel;
        let mut wish_speed = normalize(&mut wish_dir);

        let max_speed = if self.ps.stats.crouch_time != 0 {
            self.max_crouched_speed
        } else if self.cmd.buttons & BUTTON_WALK != 0 && self.feature(PMFEAT_WALK) {
            self.max_walk_speed
        } else {
            self.max_player_speed
        };
        if wish_speed > max_speed {
            wish_speed = max_speed;
        }

        if self.ladder {
            self.accelerate(wish_dir, wish_speed, PM_ACCELERATE);
            if wish_vel[2] == 0.0 {
                let gravity = self.ps.gravity as f32 * self.frame_time;
                if self.velocity[2] > 0.0 {
                    self.velocity[2] -= gravity;
                    if self.velocity[2] < 0.0 {
                        self.velocity[2] = 0.0;
                    }
                } else {
                    self.velocity[2] += gravity;
                    if self.velocity[2] > 0.0 {
                        self.velocity[2] = 0.0;
                    }
                }
            }
            self.step_slide_move();
        } else if self.ground_entity != -1 {
            if self.velocity[2] > 0.0 {
                self.velocity[2] = 0.0;
            }
            self.accelerate(wish_dir, wish_speed, PM_ACCELERATE);

            // Negative gravity fields pull the player off the ground.
            if self.ps.gravity > 0 {
                if self.velocity[2] > 0.0 {
                    self.velocity[2] = 0.0;
                }
            } else {
                self.add_gravity();
            }

            if self.velocity[0] == 0.0 && self.velocity[1] == 0.0 {
                return;
            }
            self.step_slide_move();
        } else if self.feature(PMFEAT_AIRCONTROL) && !self.feature(PMFEAT_FWDBUNNY) {
            let wish_speed2 = wish_speed;
            let mut accel = if dot(self.velocity, wish_dir) < 0.0
                && !self.flag(PMF_WALLJUMPING)
                && self.ps.stats.knockback <= 0
            {
                PM_AIRDECELERATE
            } else {
                PM_AIRACCELERATE
            };
            if self.flag(PMF_WALLJUMPING) {
                accel = 0.0;
            }
            if smove != 0.0 && fmove == 0.0 && self.ps.stats.knockback <= 0 {
                if wish_speed > PM_WISHSPEED {
                    wish_speed = PM_WISHSPEED;
                }
                accel = PM_STRAFEBUNNYACCEL;
            }

            self.accelerate(wish_dir, wish_speed, accel);
            if !self.flag(PMF_WALLJUMPING) && self.ps.stats.knockback <= 0 {
                self.air_control(wish_dir, wish_speed2);
            }

            self.add_gravity();
            self.step_slide_move();
        } else {
            let mut inhibit = false;
            let accelerating = dot(self.velocity, wish_dir) > 0.0;
            let decelerating = dot(self.velocity, wish_dir) < -0.0;

            if self.flag(PMF_WALLJUMPING)
                && self.ps.stats.wall_jump_time
                    >= PM_WALLJUMP_TIMEDELAY - PM_AIRCONTROL_BOUNCE_DELAY
            {
                inhibit = true;
            }
            if self.flag(PMF_DASHING)
                && self.ps.stats.dash_time >= PM_DASHJUMP_TIMEDELAY - PM_AIRCONTROL_BOUNCE_DELAY
            {
                inhibit = true;
            }
            if !self.feature(PMFEAT_FWDBUNNY) || self.ps.stats.forward_time > 0 {
                inhibit = true;
            }

            if accelerating && !inhibit && smove == 0.0 && fmove != 0.0 {
                self.air_accelerate(wish_dir, wish_speed);
            } else {
                let mut air_control = true;
                let wish_speed2 = wish_speed;
                let mut accel = if decelerating && !self.flag(PMF_WALLJUMPING) {
                    PM_AIRDECELERATE
                } else {
                    PM_AIRACCELERATE
                };
                if self.flag(PMF_WALLJUMPING) {
                    accel = 0.0;
                    air_control = false;
                }
                if self.flag(PMF_DASHING)
                    && self.ps.stats.dash_time >= PM_DASHJUMP_TIMEDELAY - PM_AIRCONTROL_BOUNCE_DELAY
                {
                    air_control = false;
                }
                if !self.feature(PMFEAT_AIRCONTROL) {
                    air_control = false;
                }

                if air_control && smove != 0.0 && fmove == 0.0 {
                    if wish_speed > PM_WISHSPEED {
                        wish_speed = PM_WISHSPEED;
                    }
                    self.accelerate(wish_dir, wish_speed, PM_STRAFEBUNNYACCEL);
                    self.air_control(wish_dir, wish_speed2);
                } else {
                    self.accelerate(wish_dir, wish_speed, accel);
                }
            }

            self.add_gravity();
            self.step_slide_move();
        }
    }

    fn good_position(&self, origin: Vec3) -> bool {
        if self.ps.pm_type == PmType::Spectator {
            return true;
        }
        let trace = self.trace(origin, self.ps.mins, self.ps.maxs, origin);
        trace.fraction == 1.0 && !trace.start_solid
    }

    fn calc_good_position(&self, given: Vec3) -> (CalcPosition, Vec3) {
        const SIDE_SHIFT_SIGNS: [f32; 3] = [0.0, -1.0, 1.0];
        const Z_SHIFT_SIGNS: [f32; 3] = [0.0, 1.0, -1.0];
        const SHIFT_DISTANCE: f32 = 1.0 / 8.0;
        let one_over_sqrt2 = 1.0 / 2.0f32.sqrt();
        let one_over_sqrt3 = 1.0 / 3.0f32.sqrt();

        for z in 0..3 {
            for y in 0..3 {
                for x in 0..3 {
                    let mut shift = [SIDE_SHIFT_SIGNS[x], SIDE_SHIFT_SIGNS[y], Z_SHIFT_SIGNS[z]];
                    let shifts = usize::from(x > 0) + usize::from(y > 0) + usize::from(z > 0);
                    if shifts > 0 {
                        let mut s = SHIFT_DISTANCE;
                        if shifts == 2 {
                            s *= one_over_sqrt2;
                        } else if shifts == 3 {
                            s *= one_over_sqrt3;
                        }
                        shift = scale(shift, s);
                    }
                    let shifted = [
                        given[0] + shift[0],
                        given[1] + shift[1],
                        given[2] + shift[2],
                    ];
                    if self.good_position(shifted) {
                        return (
                            if shifts > 0 {
                                CalcPosition::Corrected
                            } else {
                                CalcPosition::WasGood
                            },
                            shifted,
                        );
                    }
                }
            }
        }
        (CalcPosition::Unsolved, given)
    }

    fn unstick_position(&mut self) {
        if self.rules.skip_collision {
            return;
        }
        match self.calc_good_position(self.origin) {
            (CalcPosition::WasGood, _) => {}
            (CalcPosition::Corrected, good) => self.origin = good,
            (CalcPosition::Unsolved, _) => self.origin = self.previous_origin,
        }
    }

    fn categorize_position(&mut self) {
        if self.velocity[2] > 180.0 {
            self.ps.flags &= !PMF_ON_GROUND;
            self.ground_entity = -1;
        } else {
            let trace = if !self.rules.skip_collision {
                let point = [
                    self.origin[0],
                    self.origin[1],
                    (f64::from(self.origin[2]) - 0.25) as f32,
                ];
                self.trace(self.origin, self.ps.mins, self.ps.maxs, point)
            } else {
                PmTrace::miss(self.origin)
            };

            self.ground_normal = trace.plane_normal;
            self.ground_surface_flags = trace.surface_flags;

            if trace.fraction == 1.0 || (!is_walkable(trace.plane_normal) && !trace.start_solid) {
                self.ground_entity = -1;
                self.ps.flags &= !PMF_ON_GROUND;
            } else {
                self.ground_entity = trace.entity;

                // Hitting solid ground ends a water jump.
                if self.flag(PMF_TIME_WATERJUMP) {
                    self.ps.flags &= !(PMF_TIME_WATERJUMP | PMF_TIME_LAND | PMF_TIME_TELEPORT);
                    self.ps.pm_time = 0;
                }
                self.ps.flags |= PMF_ON_GROUND;
            }

            if self.touch.len() < MAX_TOUCH && trace.fraction < 1.0 {
                self.touch.push(trace.entity);
            }
        }

        self.water_level = 0;
        self.water_type = 0;

        let sample2 = (self.ps.view_height as f32 - self.ps.mins[2]) as i32;
        let sample1 = sample2 / 2;
        let below = self.origin[2] + self.ps.mins[2];
        let mut point = [self.origin[0], self.origin[1], below + 1.0];
        let contents = self.world.point_contents(point);
        if contents & MASK_WATER != 0 {
            self.water_type = contents;
            self.water_level = 1;
            point[2] = below + sample1 as f32;
            if self.world.point_contents(point) & MASK_WATER != 0 {
                self.water_level = 2;
                point[2] = below + sample2 as f32;
                if self.world.point_contents(point) & MASK_WATER != 0 {
                    self.water_level = 3;
                }
            }
        }
    }

    fn clear_dash(&mut self) {
        self.ps.flags &= !PMF_DASHING;
        self.ps.stats.dash_time = 0;
    }

    fn clear_wall_jump(&mut self) {
        self.ps.flags &= !(PMF_WALLJUMPING | PMF_WALLJUMPCOUNT);
        self.ps.stats.wall_jump_time = 0;
    }

    fn clip_to_ground_when_leaving(&mut self) {
        if self.ground_normal[2] > 0.0
            && self.velocity[2] < 0.0
            && dot2(self.ground_normal, self.velocity) > 0.0
        {
            self.velocity = clip_velocity(self.velocity, self.ground_normal, PM_OVERBOUNCE);
        }
    }

    fn check_jump(&mut self) {
        if self.up_push < 10.0 {
            if !self.feature(PMFEAT_CONTINOUSJUMP) {
                self.ps.flags &= !PMF_JUMP_HELD;
            }
            return;
        }
        if !self.feature(PMFEAT_CONTINOUSJUMP) && self.flag(PMF_JUMP_HELD) {
            return;
        }
        if self.ps.pm_type != PmType::Normal {
            return;
        }
        if self.water_level >= 2 {
            self.ground_entity = -1;
            return;
        }
        if self.ground_entity == -1 || !self.feature(PMFEAT_JUMP) {
            return;
        }
        if !self.feature(PMFEAT_CONTINOUSJUMP) {
            self.ps.flags |= PMF_JUMP_HELD;
        }
        self.ground_entity = -1;

        self.clip_to_ground_when_leaving();

        if self.velocity[2] > 100.0 {
            self.emit(PmEvent::DoubleJump, 0);
            self.velocity[2] += self.jump_player_speed;
        } else if self.velocity[2] > 0.0 {
            self.emit(PmEvent::Jump, 0);
            self.velocity[2] += self.jump_player_speed;
        } else {
            self.emit(PmEvent::Jump, 0);
            self.velocity[2] = self.jump_player_speed;
        }

        self.ps.flags &= !PMF_JUMPPAD_TIME;
        self.clear_dash();
        self.clear_wall_jump();
    }

    fn check_dash(&mut self) {
        if self.cmd.buttons & BUTTON_SPECIAL == 0 {
            self.ps.flags &= !PMF_SPECIAL_HELD;
        }
        if self.ps.pm_type != PmType::Normal
            || self.ps.stats.dash_time > 0
            || self.ps.stats.knockback > 0
        {
            return;
        }

        if self.cmd.buttons & BUTTON_SPECIAL != 0
            && self.ground_entity != -1
            && self.feature(PMFEAT_DASH)
        {
            if self.flag(PMF_SPECIAL_HELD) {
                return;
            }
            self.ps.flags &= !PMF_JUMPPAD_TIME;
            self.clear_wall_jump();
            self.ps.flags |= PMF_DASHING | PMF_SPECIAL_HELD;
            self.ground_entity = -1;

            self.clip_to_ground_when_leaving();

            let up_speed = if self.velocity[2] <= 0.0 {
                PM_DASHUPSPEED
            } else {
                PM_DASHUPSPEED + self.velocity[2]
            };

            let mut dash_dir = ma([0.0; 3], self.forward_push, self.flat_forward);
            dash_dir = ma(dash_dir, self.side_push, self.right);
            dash_dir[2] = 0.0;
            if length(dash_dir) < 0.01 {
                dash_dir = self.flat_forward;
            }
            normalize(&mut dash_dir);

            let actual_velocity = normalize2d(&mut self.velocity);
            dash_dir = if actual_velocity <= self.dash_player_speed {
                scale(dash_dir, self.dash_player_speed)
            } else {
                scale(dash_dir, actual_velocity)
            };

            self.velocity = dash_dir;
            self.velocity[2] = up_speed;
            self.ps.stats.dash_time = PM_DASHJUMP_TIMEDELAY;

            if self.side_push.abs() > 10.0 && self.side_push.abs() >= self.forward_push.abs() {
                self.emit(PmEvent::Dash, if self.side_push > 0.0 { 2 } else { 1 });
            } else if self.forward_push < -10.0 {
                self.emit(PmEvent::Dash, 3);
            } else {
                self.emit(PmEvent::Dash, 0);
            }
        } else if self.ground_entity == -1 {
            self.ps.flags &= !PMF_DASHING;
        }
    }

    fn check_wall_jump(&mut self) {
        if self.cmd.buttons & BUTTON_SPECIAL == 0 {
            self.ps.flags &= !PMF_SPECIAL_HELD;
        }
        if self.ground_entity != -1 {
            self.ps.flags &= !(PMF_WALLJUMPING | PMF_WALLJUMPCOUNT);
        }
        if self.flag(PMF_WALLJUMPING) && self.velocity[2] < 0.0 {
            self.ps.flags &= !PMF_WALLJUMPING;
        }
        if self.ps.stats.wall_jump_time <= 0 {
            self.ps.flags &= !PMF_WALLJUMPCOUNT;
        }
        if self.ps.pm_type != PmType::Normal {
            return;
        }
        // No wall jump during the first 100 ms of a dash jump.
        if self.flag(PMF_DASHING) && self.ps.stats.dash_time > PM_DASHJUMP_TIMEDELAY - 100 {
            return;
        }

        if self.ground_entity == -1
            && self.cmd.buttons & BUTTON_SPECIAL != 0
            && self.feature(PMFEAT_WALLJUMP)
            && !self.flag(PMF_WALLJUMPCOUNT)
            && self.ps.stats.wall_jump_time <= 0
            && !self.rules.skip_collision
        {
            let point = [self.origin[0], self.origin[1], self.origin[2] - STEP_SIZE];
            let h_speed = length([self.velocity[0], self.velocity[1], 0.0]);
            let trace = self.trace(self.origin, self.ps.mins, self.ps.maxs, point);

            // Not within a step of the ground, unless jumping or moving fast upwards.
            if self.up_push >= 10.0
                || (h_speed > self.ps.stats.dash_speed as f32 && self.velocity[2] > 8.0)
                || trace.fraction == 1.0
                || (!is_walkable(trace.plane_normal) && !trace.start_solid)
            {
                let normal = self.player_touch_wall(20, 0.3);
                if length(normal) == 0.0 {
                    return;
                }

                if !self.flag(PMF_SPECIAL_HELD) && !self.flag(PMF_WALLJUMPING) {
                    let old_up_velocity = self.velocity[2];
                    self.velocity[2] = 0.0;
                    let mut h_speed = normalize2d(&mut self.velocity);

                    if self.ps.stats.stun > 0 {
                        self.velocity = clip_velocity(self.velocity, normal, 1.0);
                        self.velocity = ma(self.velocity, PM_FAILEDWJBOUNCEFACTOR, normal);
                        normalize(&mut self.velocity);
                        self.velocity = scale(self.velocity, h_speed);
                        self.velocity[2] =
                            if old_up_velocity + PM_FAILEDWJUPSPEED > PM_FAILEDWJUPSPEED {
                                old_up_velocity
                            } else {
                                old_up_velocity + PM_FAILEDWJUPSPEED
                            };
                    } else {
                        self.velocity = clip_velocity(self.velocity, normal, 1.0005);
                        self.velocity = ma(self.velocity, PM_WJBOUNCEFACTOR, normal);
                        let min_speed = (self.max_walk_speed + self.max_player_speed) * 0.5;
                        if h_speed < min_speed {
                            h_speed = min_speed;
                        }
                        normalize(&mut self.velocity);
                        self.velocity = scale(self.velocity, h_speed);
                        self.velocity[2] = if old_up_velocity > PM_WJUPSPEED {
                            old_up_velocity
                        } else {
                            PM_WJUPSPEED
                        };
                    }

                    self.clear_dash();
                    self.ps.flags &= !PMF_JUMPPAD_TIME;
                    self.ps.flags |= PMF_WALLJUMPING | PMF_SPECIAL_HELD | PMF_WALLJUMPCOUNT;

                    let direction = i32::from(dir_to_byte(normal));
                    if self.ps.stats.stun > 0 {
                        self.ps.stats.wall_jump_time = PM_WALLJUMP_FAILED_TIMEDELAY;
                        self.emit(PmEvent::WallJumpFailed, direction);
                    } else {
                        self.ps.stats.wall_jump_time = PM_WALLJUMP_TIMEDELAY;
                        self.emit(PmEvent::WallJump, direction);
                    }
                }
            }
        } else {
            self.ps.flags &= !PMF_WALLJUMPING;
        }
    }

    fn check_crouch_slide(&mut self) {
        if !self.feature(PMFEAT_CROUCHSLIDING) {
            return;
        }
        let h_speed = length([self.velocity[0], self.velocity[1], 0.0]);
        if self.up_push < 0.0 && h_speed > self.max_walk_speed {
            if self.ps.stats.crouch_slide_time > 0 || self.ground_entity != -1 {
                return;
            }
            self.ps.flags |= PMF_CROUCH_SLIDING;
            self.ps.stats.crouch_slide_time = PM_CROUCHSLIDE + PM_CROUCHSLIDE_FADE;
        } else if self.flag(PMF_CROUCH_SLIDING) {
            self.ps.stats.crouch_slide_time =
                self.ps.stats.crouch_slide_time.min(PM_CROUCHSLIDE_FADE);
        }
    }

    fn check_special_movement(&mut self) {
        self.pm_ladder = false;
        if self.ps.pm_time != 0 {
            return;
        }
        self.ladder = false;

        if !self.rules.skip_collision && !self.rules.skip_ladders {
            let spot = ma(self.origin, 1.0, self.flat_forward);
            let trace = self.trace(self.origin, self.ps.mins, self.ps.maxs, spot);
            if trace.fraction < 1.0 && trace.surface_flags & SURF_LADDER != 0 {
                self.ladder = true;
                self.pm_ladder = true;
            }
        }

        if self.water_level != 2 {
            return;
        }
        let mut spot = ma(self.origin, 30.0, self.flat_forward);
        spot[2] += 4.0;
        if self.world.point_contents(spot) & CONTENTS_SOLID == 0 {
            return;
        }
        spot[2] += 16.0;
        if self.world.point_contents(spot) != 0 {
            return;
        }

        self.velocity = scale(self.flat_forward, 50.0);
        self.velocity[2] = 350.0;
        self.ps.flags |= PMF_TIME_WATERJUMP;
        self.ps.pm_time = 255;
    }

    fn fly_move(&mut self, clip: bool) {
        let mut max_speed = self.max_player_speed * 1.5;
        let special = self.cmd.buttons & BUTTON_SPECIAL != 0;
        if special {
            max_speed *= 2.0;
        }

        let speed = length(self.velocity);
        if speed < 1.0 {
            self.velocity = [0.0; 3];
        } else {
            let friction = PM_FRICTION * 1.5;
            let control = if speed < PM_DECELERATE {
                PM_DECELERATE
            } else {
                speed
            };
            let drop = control * friction * self.frame_time;
            let mut new_speed = speed - drop;
            if new_speed < 0.0 {
                new_speed = 0.0;
            }
            self.velocity = scale(self.velocity, new_speed / speed);
        }

        let (mut fmove, mut smove) = (self.forward_push, self.side_push);
        if special {
            fmove *= 2.0;
            smove *= 2.0;
        }
        normalize(&mut self.forward);
        normalize(&mut self.right);

        let mut wish_vel: Vec3 =
            std::array::from_fn(|i| self.forward[i] * fmove + self.right[i] * smove);
        wish_vel[2] += self.up_push;
        let mut wish_dir = wish_vel;
        let mut wish_speed = normalize(&mut wish_dir);
        if wish_speed > max_speed {
            wish_speed = max_speed;
        }

        let add_speed = wish_speed - dot(self.velocity, wish_dir);
        if add_speed > 0.0 {
            let mut accel_speed = PM_ACCELERATE * self.frame_time * wish_speed;
            if accel_speed > add_speed {
                accel_speed = add_speed;
            }
            for i in 0..3 {
                self.velocity[i] += accel_speed * wish_dir[i];
            }
        }

        if clip {
            let end = std::array::from_fn(|i| self.origin[i] + self.frame_time * self.velocity[i]);
            let trace = self.trace(self.origin, self.ps.mins, self.ps.maxs, end);
            self.origin = trace.end_position;
        } else {
            self.origin = ma(self.origin, self.frame_time, self.velocity);
        }
    }

    fn check_zoom(&mut self) {
        if self.ps.pm_type != PmType::Normal {
            self.ps.stats.zoom_time = 0;
            return;
        }
        let msec = i32::from(self.cmd.msec);
        if self.cmd.buttons & BUTTON_ZOOM != 0 && self.feature(PMFEAT_ZOOM) {
            self.ps.stats.zoom_time = (self.ps.stats.zoom_time + msec).clamp(0, ZOOM_TIME);
        } else if self.ps.stats.zoom_time > 0 {
            self.ps.stats.zoom_time = (self.ps.stats.zoom_time - msec).clamp(0, ZOOM_TIME);
        }
    }

    fn crouch_box(crouch_time: i32) -> (Vec3, Vec3, i32) {
        let fraction = crouch_time as f32 / CROUCH_TIME as f32;
        let height = PLAYER_STAND_VIEWHEIGHT as f32
            - fraction * (PLAYER_STAND_VIEWHEIGHT - PLAYER_CROUCH_VIEWHEIGHT) as f32;
        (
            lerp(PLAYER_STAND_MINS, fraction, PLAYER_CROUCH_MINS),
            lerp(PLAYER_STAND_MAXS, fraction, PLAYER_CROUCH_MAXS),
            height as i32,
        )
    }

    fn adjust_bbox(&mut self) {
        match self.ps.pm_type {
            PmType::Gib => {
                self.ps.stats.crouch_time = 0;
                self.ps.maxs = PLAYER_GIB_MAXS;
                self.ps.mins = PLAYER_GIB_MINS;
                self.ps.view_height = PLAYER_GIB_VIEWHEIGHT;
                return;
            }
            PmType::Freeze | PmType::Chasecam => {
                self.ps.stats.crouch_time = 0;
                self.ps.view_height = 0;
                return;
            }
            PmType::Spectator => {
                self.ps.stats.crouch_time = 0;
                self.ps.view_height = PLAYER_STAND_VIEWHEIGHT;
            }
            PmType::Normal => {}
        }

        let msec = i32::from(self.cmd.msec);
        if self.up_push < 0.0
            && self.feature(PMFEAT_CROUCH)
            && self.ps.stats.wall_jump_time < PM_WALLJUMP_TIMEDELAY - PM_SPECIAL_CROUCH_INHIBIT
            && self.ps.stats.dash_time < PM_DASHJUMP_TIMEDELAY - PM_SPECIAL_CROUCH_INHIBIT
        {
            self.ps.stats.crouch_time = (self.ps.stats.crouch_time + msec).clamp(0, CROUCH_TIME);
            let (mins, maxs, height) = Self::crouch_box(self.ps.stats.crouch_time);
            self.ps.mins = mins;
            self.ps.maxs = maxs;
            self.ps.view_height = height;
            return;
        }

        if self.ps.stats.crouch_time != 0 {
            let (cur_mins, cur_maxs, cur_height) = Self::crouch_box(self.ps.stats.crouch_time);
            if msec == 0 {
                self.ps.mins = cur_mins;
                self.ps.maxs = cur_maxs;
                self.ps.view_height = cur_height;
                return;
            }

            let new_time = (self.ps.stats.crouch_time - msec).clamp(0, CROUCH_TIME);
            let (wish_mins, wish_maxs, wish_height) = Self::crouch_box(new_time);

            let trace = self.trace(self.origin, wish_mins, wish_maxs, self.origin);
            if trace.all_solid || trace.start_solid {
                self.ps.mins = cur_mins;
                self.ps.maxs = cur_maxs;
                self.ps.view_height = cur_height;
                return;
            }

            self.ps.stats.crouch_time = new_time;
            self.ps.mins = wish_mins;
            self.ps.maxs = wish_maxs;
            self.ps.view_height = wish_height;
            return;
        }

        self.ps.mins = PLAYER_STAND_MINS;
        self.ps.maxs = PLAYER_STAND_MAXS;
        self.ps.view_height = PLAYER_STAND_VIEWHEIGHT;
    }

    fn apply_mouse_angles_clamp(&mut self) {
        const LIMIT: i32 = 16384;
        for i in 0..3 {
            let mut temp = i32::from(self.cmd.angles[i].wrapping_add(self.ps.delta_angles[i]));
            if i == 0 {
                if temp > LIMIT - 1 {
                    self.ps.delta_angles[i] = (LIMIT - 1 - i32::from(self.cmd.angles[i])) as i16;
                    temp = LIMIT - 1;
                } else if temp < -LIMIT + 1 {
                    self.ps.delta_angles[i] = (-LIMIT + 1 - i32::from(self.cmd.angles[i])) as i16;
                    temp = -LIMIT + 1;
                }
            }
            self.ps.view_angles[i] = short_to_angle(temp as i16);
        }

        let (forward, right, up) = angle_vectors(self.ps.view_angles);
        self.forward = forward;
        self.right = right;
        self.up = up;
        self.flat_forward = [forward[0], forward[1], 0.0];
        normalize(&mut self.flat_forward);
    }

    fn begin_move(&mut self) {
        self.touch.clear();
        self.ground_entity = -1;
        self.water_type = 0;
        self.water_level = 0;
        self.step = 0.0;

        if !self.rules.skip_collision && self.rules.snap_initially {
            let (result, good) = self.calc_good_position(self.ps.origin);
            if result == CalcPosition::Corrected {
                self.ps.origin = good;
            }
        }
        self.origin = self.ps.origin;
        self.velocity = self.ps.velocity;
        self.previous_origin = self.ps.origin;
        self.previous_velocity = self.ps.velocity;
    }

    fn touch_triggers(&mut self) {
        let Some(hook) = self.touch_hook.as_mut() else {
            return;
        };
        let context = TouchContext {
            previous_origin: self.previous_origin,
            previous_velocity: self.previous_velocity,
            command_angles: self.cmd.angles,
        };
        hook.touch_triggers(self.ps, &context, &mut self.events);
        // Triggers may have turned the view (teleport): refresh delta angles.
        for i in 0..3 {
            let view = angle_to_short(self.ps.view_angles[i]) as i16;
            self.ps.delta_angles[i] = view.wrapping_sub(self.cmd.angles[i]);
        }
    }

    fn end_move(&mut self) {
        self.ps.origin = self.origin;
        self.ps.velocity = self.velocity;
    }

    fn tick_timers(&mut self) {
        let msec = i32::from(self.cmd.msec);
        let ps = &mut *self.ps;

        if ps.pm_time != 0 {
            let step = (msec >> 3).max(1);
            if step >= ps.pm_time {
                ps.flags &= !(PMF_TIME_WATERJUMP | PMF_TIME_LAND | PMF_TIME_TELEPORT);
                ps.pm_time = 0;
            } else {
                ps.pm_time -= step;
            }
        }

        let count_down = |value: &mut i32| {
            if *value > 0 {
                *value -= msec;
            } else if *value < 0 {
                *value = 0;
            }
        };
        count_down(&mut ps.stats.no_user_control);
        count_down(&mut ps.stats.knockback);
        count_down(&mut ps.stats.dash_time);
        count_down(&mut ps.stats.wall_jump_time);
        count_down(&mut ps.stats.no_auto_attack);
        count_down(&mut ps.stats.stun);
        count_down(&mut ps.stats.forward_time);

        if ps.stats.crouch_slide_time > 0 {
            ps.stats.crouch_slide_time -= msec;
            if ps.stats.crouch_slide_time <= 0 {
                ps.stats.crouch_slide_time = if ps.flags & PMF_CROUCH_SLIDING != 0 {
                    PM_CROUCHSLIDE_TIMEDELAY
                } else {
                    0
                };
                ps.flags &= !PMF_CROUCH_SLIDING;
            }
        }
    }

    fn run(mut self) -> PmoveResult {
        self.begin_move();

        let fall_velocity = if self.velocity[2] < 0.0 {
            self.velocity[2].abs()
        } else {
            0.0
        };
        self.frame_time = (f64::from(self.cmd.msec) * 0.001) as f32;

        self.max_player_speed = self.ps.stats.max_speed as f32;
        if self.max_player_speed < 0.0 {
            self.max_player_speed = DEFAULT_PLAYER_SPEED;
        }
        self.jump_player_speed = self.ps.stats.jump_speed as f32 * GRAVITY_COMPENSATE;
        if self.jump_player_speed < 0.0 {
            self.jump_player_speed = DEFAULT_JUMP_SPEED * GRAVITY_COMPENSATE;
        }
        self.dash_player_speed = self.ps.stats.dash_speed as f32;
        if self.dash_player_speed < 0.0 {
            self.dash_player_speed = DEFAULT_DASH_SPEED;
        }
        self.max_walk_speed = DEFAULT_WALKSPEED.min(self.max_player_speed * 0.66);
        self.max_crouched_speed = DEFAULT_CROUCHEDSPEED.min(self.max_player_speed * 0.5);

        self.contents_mask = match self.ps.pm_type {
            PmType::Freeze | PmType::Chasecam => {
                self.ps.flags |= PMF_NO_PREDICTION;
                0
            }
            PmType::Gib => {
                self.ps.flags |= PMF_NO_PREDICTION;
                MASK_DEADSOLID
            }
            PmType::Spectator => {
                self.ps.flags &= !PMF_NO_PREDICTION;
                MASK_DEADSOLID
            }
            PmType::Normal => {
                self.ps.flags &= !PMF_NO_PREDICTION;
                if self.feature(PMFEAT_GHOSTMOVE) {
                    MASK_DEADSOLID
                } else {
                    MASK_PLAYERSOLID
                }
            }
        };

        if !self.rules.match_paused {
            self.tick_timers();
        }

        self.forward_push = f32::from(self.cmd.forward_move) * SPEEDKEY / 127.0;
        self.side_push = f32::from(self.cmd.side_move) * SPEEDKEY / 127.0;
        self.up_push = f32::from(self.cmd.up_move) * SPEEDKEY / 127.0;

        if self.ps.stats.no_user_control > 0 {
            self.forward_push = 0.0;
            self.side_push = 0.0;
            self.up_push = 0.0;
            self.cmd.buttons = 0;
        }

        // Forward acceleration needs +forward held without strafing for a while.
        if self.forward_push <= 0.0 || self.side_push != 0.0 {
            self.ps.stats.forward_time = PM_FORWARD_ACCEL_TIMEDELAY;
        }

        if self.ps.pm_type != PmType::Normal {
            if !self.rules.match_paused {
                self.clear_dash();
                self.clear_wall_jump();
                self.ps.stats.stun = 0;
                self.ps.stats.knockback = 0;
                self.ps.stats.crouch_time = 0;
                self.ps.stats.zoom_time = 0;
                self.ps.flags &= !(PMF_JUMPPAD_TIME
                    | PMF_DOUBLEJUMPED
                    | PMF_TIME_WATERJUMP
                    | PMF_TIME_LAND
                    | PMF_TIME_TELEPORT
                    | PMF_SPECIAL_HELD);
                self.adjust_bbox();
            }

            if self.ps.pm_type == PmType::Spectator {
                self.apply_mouse_angles_clamp();
                self.fly_move(false);
            } else {
                self.forward_push = 0.0;
                self.side_push = 0.0;
                self.up_push = 0.0;
            }

            self.unstick_position();
            self.end_move();
            return self.finish();
        }

        self.apply_mouse_angles_clamp();
        self.adjust_bbox();
        self.check_zoom();
        self.categorize_position();

        let old_ground_entity = self.ground_entity;
        self.check_special_movement();

        if self.flag(PMF_TIME_TELEPORT) {
            // A teleport pause stays exactly in place.
        } else if self.flag(PMF_TIME_WATERJUMP) {
            self.add_gravity();
            if self.velocity[2] < 0.0 {
                self.ps.flags &= !(PMF_TIME_WATERJUMP | PMF_TIME_LAND | PMF_TIME_TELEPORT);
                self.ps.pm_time = 0;
            }
            self.step_slide_move();
        } else {
            // The order of these steps is part of the movement behaviour.
            self.check_jump();
            self.check_dash();
            self.check_wall_jump();
            self.check_crouch_slide();
            self.friction();

            if self.water_level >= 2 {
                self.water_move();
            } else {
                let mut angles = self.ps.view_angles;
                if angles[0] > 180.0 {
                    angles[0] -= 360.0;
                }
                angles[0] /= 3.0;
                let (forward, right, up) = angle_vectors(angles);
                self.forward = forward;
                self.right = right;
                self.up = up;

                // Looking straight up or down needs a special flat direction.
                self.flat_forward = if forward[2] == -1.0 {
                    up
                } else if forward[2] == 1.0 {
                    [-up[0], -up[1], -up[2]]
                } else {
                    forward
                };
                self.flat_forward[2] = 0.0;
                normalize(&mut self.flat_forward);

                self.move_walk();
            }
        }

        self.unstick_position();
        self.categorize_position();
        self.end_move();
        self.touch_triggers();

        // Mirrors the legacy "triggers may force ground off" guard.
        if !self.flag(PMF_ON_GROUND) && self.ground_entity != -1 {
            self.ground_entity = -1;
            self.velocity[2] = 0.0;
        }

        if self.ground_entity != -1 {
            // Keep the dash flag for at least 50 ms so a fresh dash is not removed.
            if self.ps.stats.dash_time < PM_DASHJUMP_TIMEDELAY - 50 {
                self.ps.flags &= !PMF_DASHING;
            }
            if self.ps.stats.wall_jump_time < PM_WALLJUMP_TIMEDELAY - 50 {
                self.clear_wall_jump();
            }
        }

        if old_ground_entity == -1 {
            let mut fall_delta = fall_velocity
                - if self.velocity[2] < 0.0 {
                    self.velocity[2].abs()
                } else {
                    0.0
                };
            match self.water_level {
                3 => fall_delta = 0.0,
                2 => fall_delta *= 0.25,
                1 => fall_delta *= 0.5,
                _ => {}
            }

            if fall_delta > FALL_STEP_MIN_DELTA {
                let damage = if !self.rules.fall_damage
                    || self.ground_surface_flags & SURF_NODAMAGE != 0
                    || self.flag(PMF_JUMPPAD_TIME)
                {
                    0.0
                } else {
                    (((fall_delta - FALL_DAMAGE_MIN_DELTA) / 10.0) * FALL_DAMAGE_SCALE)
                        .clamp(0.0, MAX_FALLING_DAMAGE)
                };
                self.emit(PmEvent::Fall, damage as i32);
            }
            self.ps.flags &= !PMF_JUMPPAD_TIME;
        }

        self.finish()
    }

    fn finish(self) -> PmoveResult {
        PmoveResult {
            events: self.events,
            touch_entities: self.touch,
            ground_entity: (self.ground_entity != -1).then_some(self.ground_entity),
            water_level: self.water_level,
            water_type: self.water_type,
            ladder: self.pm_ladder,
            step: self.step,
        }
    }
}
