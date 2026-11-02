//! `bevy_ecs` components and resources behind [`super::MapEntities`].
//!
//! None of these types leave the module: callers only see the engine-owned
//! interface in `mod.rs`.

use bevy_ecs::prelude::*;

use super::{Destination, Occupant, TriggerEvent};
use crate::game::vec::Rng;
use crate::simulation::player::TICK_DURATION;
use crate::simulation::pmove::{PmoveState, PredictedEvent, TouchContext, Vec3};

/// Position of the entity in the map's entity lump (the legacy edict number
/// for map entities).
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct LumpIndex(pub usize);

/// Lower-cased classname.
#[derive(Component, Clone, Debug)]
pub(super) struct Classname(pub String);

/// Keys read by `G_UseTargets`.
#[derive(Component, Clone, Debug, Default)]
pub(super) struct Links {
    pub targetname: Option<String>,
    pub target: Option<String>,
    pub killtarget: Option<String>,
    pub message: Option<String>,
    pub delay_ms: i64,
}

/// Spawn-time origin and angles (`s.origin`, `s.angles`).
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct Placement {
    pub origin: Vec3,
    pub angles: Vec3,
}

/// Brush model volume of an entity.
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct Volume {
    pub model: usize,
    pub origin: Vec3,
    /// Local model bounds (`r.mins`, `r.maxs`).
    pub mins: Vec3,
    pub maxs: Vec3,
    /// Link bounds: origin + bounds padded by one unit (`GClip_LinkEntity`).
    pub absmin: Vec3,
    pub absmax: Vec3,
}

/// `wait` and `timeStamp` of `G_TriggerWait`.
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct Timing {
    pub wait_s: f32,
    pub ready_after: i64,
}

/// Whether a touchable volume is solid to the touch search (`SOLID_TRIGGER`).
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct Enabled(pub bool);

/// What happens when the player touches the volume.
#[derive(Component, Clone, Copy, Debug)]
pub(super) enum TouchKind {
    Push {
        velocity: Vec3,
        once: bool,
    },
    Teleport {
        destination: Destination,
        spectator_only: bool,
    },
    Hurt {
        damage: i32,
        kill: bool,
        no_protection: bool,
    },
    Multi {
        not_player: bool,
    },
    /// Door trigger field or plat centre trigger of the mover at `owner`.
    MoverTrigger {
        owner: usize,
        plat: bool,
    },
}

/// What happens when another entity fires this one (`ent->use`).
#[derive(Component, Clone, Copy, Debug)]
pub(super) enum UseKind {
    /// `Use_Multi`.
    Multi,
    /// `trigger_enable`: a `TRIGGERED` multiple becomes touchable and usable.
    Enable,
    /// `trigger_relay_use` / `target_relay_use`.
    Relay,
    /// `target_delay_use`.
    Delay { wait_s: f32 },
    /// `target_kill_use`.
    Kill,
    /// `target_teleporter_use`.
    Teleporter { spectator_only: bool },
    /// `trigger_counter_use`.
    Counter { remaining: i32, silent: bool },
    /// `hurt_use` with the TOGGLE flag.
    HurtToggle,
    /// `target_print_use`: centre-prints the entity's `message`.
    Print { silent: bool },
    /// `use_killbox`.
    Killbox,
    /// The `use` of the brush mover at this index of the mover list.
    Mover(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ThinkKind {
    /// `trigger_always_think`.
    Always,
    /// `Think_Delay`, for the temporary `delayed_use` entity.
    DelayedUse,
    /// `target_delay_think`.
    Delay,
}

/// `nextThink` / `think`.
#[derive(Component, Clone, Copy, Debug)]
pub(super) struct Think {
    pub due_ms: i64,
    pub kind: ThinkKind,
}

/// Lump entities by index; `None` for entities the legacy game frees at spawn.
#[derive(Resource, Default)]
pub(super) struct Registry {
    pub by_lump: Vec<Option<Entity>>,
}

/// `level.time` and `level.gravity`. The only clock of the entity world: `tick` is
/// always derived from `time_ms` by [`Level::set_time`].
#[derive(Resource, Default)]
pub(super) struct Level {
    pub time_ms: i64,
    pub tick: u64,
    pub gravity: f32,
}

impl Level {
    pub fn set_time(&mut self, time_ms: i64) {
        self.time_ms = time_ms;
        self.tick = u64::try_from(time_ms / TICK_DURATION.as_millis() as i64).unwrap_or(0);
    }
}

/// Match seed; consumers draw per-`(tick, purpose)` substreams from it.
#[derive(Resource, Default)]
pub(super) struct SimRng {
    pub seed: u64,
}

impl SimRng {
    pub fn stream(&self, tick: u64, purpose: u32) -> Rng {
        Rng::substream(self.seed, tick, purpose)
    }
}

/// The local player while a tick is processed; `None` between ticks.
#[derive(Clone, Copy, Debug)]
pub(super) struct PlayerPresence {
    pub state: PmoveState,
    pub context: TouchContext,
}

#[derive(Resource, Default)]
pub(super) struct Player(pub Option<PlayerPresence>);

/// Other solid players, supplied by the caller for telefragging.
#[derive(Resource, Default)]
pub(super) struct Occupants(pub Vec<Occupant>);

/// Entities the player touched this tick, in entity number order.
#[derive(Resource, Default)]
pub(super) struct PendingTouches(pub Vec<Entity>);

/// Output of a tick.
#[derive(Resource, Default)]
pub(super) struct Outbox {
    pub events: Vec<TriggerEvent>,
    pub predicted: Vec<PredictedEvent>,
}
