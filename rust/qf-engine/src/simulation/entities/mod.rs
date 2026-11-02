//! Map entities that react to the player: jump pads, teleporters, hurt volumes,
//! generic triggers and the target entities they fire.
//!
//! Behaviour follows the legacy game module (`g_trigger.cpp`, `g_target.cpp`,
//! `g_utils.cpp:G_UseTargets/KillBox`, `client.cpp:G_TeleportPlayer`,
//! `g_clip.cpp:G_PMoveTouchTriggers` and `gs_misc.cpp:GS_TouchPushTrigger`).
//! The system is independent of rendering and windowing and runs from the
//! fixed-tick movement through [`PmoveTouch`], so a server and client
//! prediction can share it.
//!
//! Every lump entity is an entity of a private `bevy_ecs` world held by
//! [`MapEntities`]; no Bevy type is part of the public interface.
//!
//! Known differences from the legacy game:
//! - `G_PickTarget` chooses randomly among same-named targets, and
//!   `target_delay` applies a random jitter; this picks the first target and
//!   ignores the jitter, so the simulation stays deterministic.
//! - A jump pad whose target is not above it (NaN or zero airtime) is removed.
//! - The local player is the only activator. Player-dependent uses
//!   (`target_kill`, `target_teleporter`) act only while a player is present,
//!   so they are lost during a [`MapEntities::think`] without one.
//! - `KillBox` takes the other solid players from [`MapEntities::set_occupants`]
//!   and reports every overlap as a [`TriggerEvent::Telefrag`]; the world-solid
//!   early-out of the legacy box trace is not ported.
//! - No health, team, race or `trigger_relay` RANDOM logic: damage and kills
//!   are reported as events, `START_OFF` volumes stay off until used.
//! - Thinks run at the start of the tick's touch pass rather than later in
//!   `G_RunFrame`.

use std::collections::BTreeMap;
use std::fmt;

use bevy_ecs::prelude::World;

use crate::content::bsp::entities::Entity;
use crate::content::bsp::{ModelPose, Trace, TracePlane};

const CONTENT_SOLID: u32 = super::pmove::CONTENTS_SOLID;

use super::player::StepError;
#[cfg(test)]
use super::pmove::{PMF_ON_GROUND, PMF_TIME_TELEPORT, PmEvent, PmType};
use super::pmove::{PmoveState, PmoveTouch, PmoveWorld, PredictedEvent, TouchContext, Vec3};
use super::walk::{TickOutcome, WalkCommand, WalkPlayer};
use crate::game::{Inventory, PlayerInput, Projectile, WeaponEvent};

mod components;
#[cfg(test)]
mod determinism_tests;
mod hash;
mod logic;
#[cfg(test)]
mod mover_tests;
mod movers;
mod parse;
#[cfg(test)]
mod player_tests;
mod players;
mod schedule;
mod spawn;
#[cfg(test)]
mod tests;
mod weapons;

#[cfg(test)]
use parse::atof;

pub use movers::{MoverWorld, PushedPlayer};

use components::{Classname, Level, LumpIndex, Occupants, Registry, TouchKind};

/// Legacy `g_gravity` default, used when worldspawn has no `gravity` key.
pub const DEFAULT_LEVEL_GRAVITY: f32 = 850.0;

/// Brush movers stepped by [`MapEntities::step_movers`].
pub const SUPPORTED_MOVERS: &[&str] = &[
    "func_door",
    "func_door_rotating",
    "func_plat",
    "func_button",
    "func_rotating",
];

/// Classnames with a volume or a think that fire targets.
pub const SUPPORTED_TRIGGERS: &[&str] = &[
    "trigger_push",
    "trigger_teleport",
    "trigger_hurt",
    "trigger_multiple",
    "trigger_once",
    "trigger_counter",
    "trigger_always",
    "trigger_relay",
];
/// Classnames fired by triggers.
pub const SUPPORTED_ACTIONS: &[&str] = &[
    "target_relay",
    "target_delay",
    "target_kill",
    "target_print",
    "target_teleporter",
    "func_killbox",
];
/// Classnames used only as aiming points or destinations, or without effect.
pub const SUPPORTED_TARGETS: &[&str] = &[
    "target_position",
    "info_notnull",
    "target_push",
    "misc_teleporter_dest",
    "info_teleport_destination",
    // Only sets `edict_t::gravity`, which pmove never reads.
    "trigger_gravity",
];

fn is_inert(classname: &str) -> bool {
    classname == "worldspawn" || SUPPORTED_TARGETS.contains(&classname)
}

/// Brush-model queries the triggers need; implemented by the parsed BSP.
pub trait BrushModels {
    /// Local bounds of inline model `index` (`*index` in entity strings).
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)>;
    /// Whether an absolute box touches the model translated by `origin`.
    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool;
    /// `CM_TransformedBoxTrace` against inline model `index` at `pose`, for a
    /// `(start, end, mins, maxs)` sweep. `None` when the model cannot be hit.
    ///
    /// The default sweeps against the model's bounding box, ignoring rotation.
    fn trace_model(
        &self,
        index: usize,
        pose: &ModelPose,
        sweep: (Vec3, Vec3, Vec3, Vec3),
        _mask: u32,
    ) -> Option<Trace> {
        let (bmins, bmaxs) = self.model_bounds(index)?;
        let (start, end, mins, maxs) = sweep;
        Some(sweep_box(
            std::array::from_fn(|axis| bmins[axis] + pose.origin[axis] - maxs[axis]),
            std::array::from_fn(|axis| bmaxs[axis] + pose.origin[axis] - mins[axis]),
            start,
            end,
        ))
    }
}

/// Ray against the open box `lo..hi`, with the legacy 1/32 unit clip epsilon.
fn sweep_box(lo: Vec3, hi: Vec3, start: Vec3, end: Vec3) -> Trace {
    const EPSILON: f32 = 1.0 / 32.0;
    let miss = Trace {
        fraction: 1.0,
        end_position: end,
        plane: None,
        contents: 0,
        surface_flags: 0,
        shader: None,
        start_solid: false,
        all_solid: false,
    };
    let inside = |point: Vec3| (0..3).all(|axis| point[axis] > lo[axis] && point[axis] < hi[axis]);
    if inside(start) {
        return Trace {
            fraction: 0.0,
            end_position: start,
            contents: CONTENT_SOLID,
            start_solid: true,
            all_solid: inside(end),
            ..miss
        };
    }
    let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
    let mut normal = [0.0; 3];
    for axis in 0..3 {
        let delta = end[axis] - start[axis];
        if delta == 0.0 {
            if start[axis] <= lo[axis] || start[axis] >= hi[axis] {
                return miss;
            }
            continue;
        }
        let (near, far, sign) = if delta > 0.0 {
            (lo[axis], hi[axis], -1.0)
        } else {
            (hi[axis], lo[axis], 1.0)
        };
        let t_near = (near - start[axis]) / delta;
        let t_far = (far - start[axis]) / delta;
        if t_near > enter {
            enter = t_near;
            normal = [0.0; 3];
            normal[axis] = sign;
        }
        leave = leave.min(t_far);
    }
    if enter >= leave || !(0.0..1.0).contains(&enter) {
        return miss;
    }
    let length = length_of(sub_v(end, start));
    let fraction = (enter - EPSILON / length.max(EPSILON)).max(0.0);
    Trace {
        fraction,
        end_position: std::array::from_fn(|axis| {
            start[axis] + (end[axis] - start[axis]) * fraction
        }),
        plane: Some(TracePlane { normal, dist: 0.0 }),
        contents: CONTENT_SOLID,
        ..miss
    }
}

fn sub_v(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn length_of(a: Vec3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

impl BrushModels for crate::content::bsp::Bsp {
    fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
        self.models.get(index).map(|model| (model.mins, model.maxs))
    }

    fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        // MASK_ALL: trigger brushes carry no solid contents.
        self.box_touches_model(index, origin, mins, maxs, u32::MAX)
            .unwrap_or(false)
    }

    fn trace_model(
        &self,
        index: usize,
        pose: &ModelPose,
        sweep: (Vec3, Vec3, Vec3, Vec3),
        mask: u32,
    ) -> Option<Trace> {
        let (start, end, mins, maxs) = sweep;
        self.trace_model(index, pose, (start, end, mins, maxs), mask)
            .ok()
    }
}

/// Gametype facts `G_CanSpawnEntity` filters entities by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gametype {
    /// Matched case-insensitively against `gametype` / `not_gametype` lists.
    pub name: String,
    pub team_based: bool,
    pub max_players_in_team: i32,
}

impl Gametype {
    pub fn free_for_all() -> Self {
        Self {
            name: "dm".into(),
            team_based: false,
            max_players_in_team: 0,
        }
    }

    /// Bomb and Defuse: team based, several players per team.
    pub fn bomb() -> Self {
        Self {
            name: "bomb".into(),
            team_based: true,
            max_players_in_team: 5,
        }
    }
}

/// Where a teleporter sends the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Destination {
    pub origin: Vec3,
    /// Pitch (positive down), yaw, roll in the legacy convention.
    pub angles: Vec3,
}

/// Another solid player that a teleport or `func_killbox` can telefrag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Occupant {
    pub id: u32,
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
}

/// Who a telefrag kills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Victim {
    /// The local player.
    Player,
    /// An [`Occupant`] by its id.
    Occupant(u32),
}

/// Something that happened because of the entities. `entity` fields are
/// positions in the map's entity lump.
#[derive(Clone, Debug, PartialEq)]
pub enum TriggerEvent {
    JumpPad {
        entity: usize,
        velocity: Vec3,
    },
    Teleport {
        entity: usize,
        destination: Destination,
        velocity: Vec3,
    },
    /// Damage to apply to the player; `kill` means lethal regardless of health.
    Hurt {
        entity: usize,
        damage: i32,
        kill: bool,
        no_protection: bool,
    },
    /// `source` fired `target` through its `target` key.
    Used {
        source: usize,
        target: usize,
    },
    /// Centre-print text from a `message` key or a counter.
    Message {
        entity: usize,
        text: String,
    },
    /// `target_kill`: the local player dies, regardless of health.
    Kill {
        entity: usize,
    },
    /// `KillBox` found `victim` inside the box of `entity`.
    Telefrag {
        entity: usize,
        victim: Victim,
    },
    /// A mover pushed against the player and deals crush damage (`MOD_CRUSH`).
    Crush {
        entity: usize,
        damage: i32,
    },
}

/// Current placement of a brush mover; model `model` is drawn and collided at
/// `origin` rotated by `angles` (pitch positive down, yaw, roll).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverPose {
    /// Position in the entity lump, also the entity number of its trace hits.
    pub entity: usize,
    pub model: usize,
    pub origin: Vec3,
    pub angles: Vec3,
}

impl MoverPose {
    /// Maps a point in the model's own space to world space.
    pub fn local_to_world(&self, local: Vec3) -> Vec3 {
        let [forward, left, up] = movers::axis_rows(self.angles);
        std::array::from_fn(|i| {
            self.origin[i] + forward[i] * local[0] + left[i] * local[1] + up[i] * local[2]
        })
    }
}

/// Why an entity with a supported classname was not instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnWarning {
    pub entity: usize,
    pub classname: String,
    pub reason: &'static str,
}

/// The simulated map entities.
pub struct MapEntities {
    world: World,
    warnings: Vec<SpawnWarning>,
    unsupported: BTreeMap<String, usize>,
}

impl fmt::Debug for MapEntities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapEntities")
            .field("triggers", &self.trigger_count())
            .field("warnings", &self.warnings)
            .field("unsupported", &self.unsupported)
            .finish()
    }
}

impl MapEntities {
    /// Instantiates supported entities from a parsed entity lump for a
    /// free-for-all game (`notfree` entities are dropped).
    pub fn spawn<M: BrushModels + ?Sized>(entities: &[Entity], models: &M) -> Self {
        Self::spawn_for_gametype(entities, models, &Gametype::free_for_all())
    }

    /// Like [`Self::spawn`] with `G_CanSpawnEntity` filtering for `gametype`.
    pub fn spawn_for_gametype<M: BrushModels + ?Sized>(
        entities: &[Entity],
        models: &M,
        gametype: &Gametype,
    ) -> Self {
        let spawned = spawn::spawn_all(entities, models, gametype);
        Self {
            world: spawned.world,
            warnings: spawned.warnings,
            unsupported: spawned.unsupported,
        }
    }

    pub fn level_gravity(&self) -> f32 {
        self.world.resource::<Level>().gravity
    }

    /// Fixed ticks of level time seen by the entities so far.
    pub fn tick(&self) -> u64 {
        self.world.resource::<Level>().tick
    }

    /// Sets the match seed that [`Self::rng_stream`] derives from.
    pub fn set_seed(&mut self, seed: u64) {
        self.world.resource_mut::<components::SimRng>().seed = seed;
    }

    /// Deterministic generator for `(current tick, purpose)`; independent of any
    /// other consumer's draws.
    pub fn rng_stream(&self, purpose: u32) -> crate::game::vec::Rng {
        let tick = self.tick();
        self.world
            .resource::<components::SimRng>()
            .stream(tick, purpose)
    }

    /// Hash of all simulated entity state, independent of spawn layout and query order.
    pub fn state_hash(&self) -> u64 {
        hash::state_hash(&self.world)
    }

    /// Entities with a supported classname that could not be created.
    pub fn warnings(&self) -> &[SpawnWarning] {
        &self.warnings
    }

    /// Classnames present in the map that are not simulated, with counts.
    pub fn unsupported_classnames(&self) -> &BTreeMap<String, usize> {
        &self.unsupported
    }

    /// Number of live entities with a touchable volume.
    pub fn trigger_count(&self) -> usize {
        self.live()
            .filter(|&entity| self.world.get::<TouchKind>(entity).is_some())
            .count()
    }

    /// Jump pad launch velocity of lump entity `entity`, when it is a live pad.
    pub fn push_velocity(&self, entity: usize) -> Option<Vec3> {
        let entity = self
            .world
            .resource::<Registry>()
            .by_lump
            .get(entity)?
            .as_ref()?;
        match self.world.get::<TouchKind>(*entity)? {
            TouchKind::Push { velocity, .. } => Some(*velocity),
            _ => None,
        }
    }

    /// Lump positions of live entities with the given classname.
    pub fn triggers_of(&self, classname: &str) -> Vec<usize> {
        self.world
            .resource::<Registry>()
            .by_lump
            .iter()
            .enumerate()
            .filter_map(|(index, entity)| Some((index, (*entity)?)))
            .filter(|&(_, entity)| {
                self.world
                    .get::<Classname>(entity)
                    .is_some_and(|name| name.0 == classname)
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn live(&self) -> impl Iterator<Item = bevy_ecs::entity::Entity> + '_ {
        self.world
            .resource::<Registry>()
            .by_lump
            .iter()
            .flatten()
            .copied()
            .filter(|&entity| self.world.get::<LumpIndex>(entity).is_some())
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&mut self) -> Vec<TriggerEvent> {
        std::mem::take(&mut self.world.resource_mut::<components::Outbox>().events)
    }

    /// Replaces the other solid players that telefrags can hit.
    pub fn set_occupants(&mut self, occupants: Vec<Occupant>) {
        self.world.resource_mut::<Occupants>().0 = occupants;
    }

    /// Runs due thinks (`trigger_always`, `target_delay`, delayed uses) for a
    /// tick without a moving player.
    pub fn think(&mut self, time_ms: i64) {
        self.world.resource_mut::<Level>().set_time(time_ms);
        self.world.resource_mut::<components::Player>().0 = None;
        self.world.resource_mut::<schedule::Run>().thinks = true;
        schedule::run(&mut self.world, None, None);
    }

    /// Fires lump entity `entity` as if another entity had targeted it.
    /// Returns whether it has a use function.
    pub fn activate(&mut self, entity: usize, time_ms: i64) -> bool {
        let Some(Some(target)) = self
            .world
            .resource::<Registry>()
            .by_lump
            .get(entity)
            .copied()
        else {
            return false;
        };
        if self.world.get::<components::UseKind>(target).is_none() {
            return false;
        }
        self.world.resource_mut::<Level>().set_time(time_ms);
        self.world.resource_mut::<components::Player>().0 = None;
        logic::use_entity(&mut self.world, target);
        true
    }

    /// Current pose of every brush mover, in lump order.
    pub fn mover_poses(&self) -> Vec<MoverPose> {
        movers::poses(&self.world)
    }

    /// Runs the movers for one tick, after the player's movement: thinks and
    /// pushers in legacy order. `player` is carried, pushed or crushed.
    pub fn step_movers(
        &mut self,
        models: &dyn BrushModels,
        world: &dyn PmoveWorld,
        time_ms: i64,
        player: Option<PushedPlayer<'_>>,
    ) {
        self.world.resource_mut::<components::Player>().0 = None;
        self.world.resource_mut::<Level>().set_time(time_ms);
        self.world.resource_mut::<schedule::Run>().movers = true;
        let input = player.as_ref().map(|p| (*p.state, p.ground_entity));
        self.world.resource_mut::<schedule::MoverFrame>().player = input;
        schedule::run(&mut self.world, Some(models), Some(world));
        let output = self
            .world
            .resource_mut::<schedule::MoverFrame>()
            .player
            .take();
        if let (Some(player), Some((state, _))) = (player, output) {
            *player.state = state;
        }
    }

    /// Adds the player of `slot`, or replaces its movement state and rules.
    pub fn set_player(&mut self, slot: u8, player: WalkPlayer) {
        players::spawn(&mut self.world, slot, player);
    }

    /// Removes the player of `slot`; returns whether there was one.
    pub fn remove_player(&mut self, slot: u8) -> bool {
        players::despawn(&mut self.world, slot)
    }

    /// Queues the usercmd the player of `slot` applies on the next tick.
    pub fn push_input(&mut self, slot: u8, command: WalkCommand) {
        if let Some(entity) = players::entity_of(&mut self.world, slot) {
            self.world.entity_mut(entity).insert(players::Cmd(command));
        }
    }

    /// Movement events of every player since the last call, in slot order.
    pub fn take_player_events(&mut self) -> Vec<(u8, PredictedEvent)> {
        let mut out = Vec::new();
        for (slot, entity) in players::in_slot_order(&mut self.world) {
            if let Some(mut walker) = self.world.get_mut::<players::Walker>(entity) {
                out.extend(walker.0.events.drain(..).map(|event| (slot, event)));
            }
        }
        out
    }

    /// Queues the weapon buttons the player of `slot` applies on the next tick. The
    /// attack button is level-triggered; select and cycle are consumed once.
    pub fn push_weapon_input(&mut self, slot: u8, input: PlayerInput) {
        if let Some(entity) = players::entity_of(&mut self.world, slot) {
            self.world
                .entity_mut(entity)
                .insert(weapons::WeaponCmd(input));
        }
    }

    /// Weapon inventory of the player of `slot`.
    pub fn inventory_mut(&mut self, slot: u8) -> Option<&mut Inventory> {
        let entity = players::entity_of(&mut self.world, slot)?;
        self.world
            .get_mut::<Inventory>(entity)
            .map(|inventory| inventory.into_inner())
    }

    pub fn inventory(&mut self, slot: u8) -> Option<&Inventory> {
        let entity = players::entity_of(&mut self.world, slot)?;
        self.world.get::<Inventory>(entity)
    }

    /// Dead players neither fire nor can be hit.
    pub fn set_alive(&mut self, slot: u8, alive: bool) {
        if let Some(entity) = players::entity_of(&mut self.world, slot) {
            self.world.entity_mut(entity).insert(weapons::Alive(alive));
        }
    }

    /// Weapon events (shots, impacts, damage) since the last call.
    pub fn take_weapon_events(&mut self) -> Vec<WeaponEvent> {
        weapons::take_events(&mut self.world)
    }

    /// Projectiles in flight, in spawn order.
    pub fn projectiles(&self) -> Vec<Projectile> {
        weapons::shots(&self.world)
    }

    /// Slots of all players, ascending.
    pub fn player_slots(&mut self) -> Vec<u8> {
        players::in_slot_order(&mut self.world)
            .into_iter()
            .map(|(slot, _)| slot)
            .collect()
    }

    /// Movement state of the player of `slot`.
    pub fn player(&mut self, slot: u8) -> Option<&WalkPlayer> {
        let entity = players::entity_of(&mut self.world, slot)?;
        self.world.get::<players::Walker>(entity).map(|w| &w.0)
    }

    /// What the last tick fed to and got back from `pmove` for `slot`.
    pub fn last_outcome(&mut self, slot: u8) -> Option<&TickOutcome> {
        let entity = players::entity_of(&mut self.world, slot)?;
        self.world.get::<players::LastTick>(entity)?.0.as_ref()
    }

    /// One fixed tick of the schedule for every player entity: usercmd movement with
    /// trigger touching in slot order, then the movers (carrying the lowest slot).
    pub fn tick_players(
        &mut self,
        collision: &dyn PmoveWorld,
        models: &dyn BrushModels,
        time_ms: i64,
    ) -> Result<(), StepError> {
        self.world.resource_mut::<Level>().set_time(time_ms);
        self.world.resource_mut::<components::Player>().0 = None;
        self.world.resource_mut::<players::Fault>().0 = None;
        {
            let mut run = self.world.resource_mut::<schedule::Run>();
            run.players = true;
            run.movers = true;
        }
        schedule::run(&mut self.world, Some(models), Some(collision));
        match self.world.resource_mut::<players::Fault>().0.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// of the pmove result): walking into a solid button presses it.
    pub fn player_touched(&mut self, entities: &[i32], time_ms: i64) {
        for &entity in entities {
            if let Ok(lump) = usize::try_from(entity) {
                movers::touched_by_player(&mut self.world, lump, time_ms);
            }
        }
    }

    /// Damages a shootable door or button (`health` key). Returns whether the
    /// entity is a shootable mover. Weapons are not simulated, so callers
    /// provide the damage.
    pub fn damage_mover(&mut self, entity: usize, damage: i32, time_ms: i64) -> bool {
        movers::shoot(&mut self.world, entity, damage, time_ms)
    }

    /// Binds the entities to a world and a level time for one movement tick.
    pub fn player_touch<'a, M: BrushModels>(
        &'a mut self,
        models: &'a M,
        time_ms: i64,
    ) -> PlayerTouch<'a, M> {
        PlayerTouch {
            entities: self,
            models,
            time_ms,
        }
    }

    fn touch(
        &mut self,
        models: &dyn BrushModels,
        time_ms: i64,
        state: &mut PmoveState,
        context: &TouchContext,
        predicted: &mut Vec<PredictedEvent>,
    ) {
        self.world.resource_mut::<Level>().set_time(time_ms);
        self.world.resource_mut::<components::Player>().0 = Some(components::PlayerPresence {
            state: *state,
            context: *context,
        });
        self.world.resource_mut::<schedule::TouchFrame>().swept =
            logic::swept_bounds(state, context.previous_origin);
        self.world.resource_mut::<schedule::Run>().touch = true;
        schedule::run(&mut self.world, Some(models), None);
        if let Some(player) = schedule::take_player(&mut self.world) {
            *state = player.state;
        }
        schedule::take_predicted(&mut self.world, predicted);
    }
}

/// [`PmoveTouch`] adapter for one movement tick.
pub struct PlayerTouch<'a, M: BrushModels> {
    entities: &'a mut MapEntities,
    models: &'a M,
    time_ms: i64,
}

impl<M: BrushModels> PmoveTouch for PlayerTouch<'_, M> {
    fn touch_triggers(
        &mut self,
        state: &mut PmoveState,
        context: &TouchContext,
        events: &mut Vec<PredictedEvent>,
    ) {
        self.entities
            .touch(self.models, self.time_ms, state, context, events);
    }
}
