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

#[cfg(test)]
use super::pmove::{PMF_ON_GROUND, PMF_TIME_TELEPORT, PmEvent, PmType};
use super::pmove::{PmoveState, PmoveTouch, PredictedEvent, TouchContext, Vec3};

mod components;
mod logic;
mod parse;
mod spawn;
#[cfg(test)]
mod tests;

#[cfg(test)]
use parse::atof;

use components::{Classname, Level, LumpIndex, Occupants, Registry, TouchKind};

/// Legacy `g_gravity` default, used when worldspawn has no `gravity` key.
pub const DEFAULT_LEVEL_GRAVITY: f32 = 850.0;

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
        self.world.resource_mut::<Level>().time_ms = time_ms;
        self.world.resource_mut::<components::Player>().0 = None;
        logic::run_thinks(&mut self.world);
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
        self.world.resource_mut::<Level>().time_ms = time_ms;
        self.world.resource_mut::<components::Player>().0 = None;
        logic::use_entity(&mut self.world, target);
        true
    }

    /// Binds the entities to a world and a level time for one movement tick.
    pub fn player_touch<'a, M: BrushModels + ?Sized>(
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
        models: &(impl BrushModels + ?Sized),
        time_ms: i64,
        state: &mut PmoveState,
        context: &TouchContext,
        predicted: &mut Vec<PredictedEvent>,
    ) {
        logic::touch(&mut self.world, models, time_ms, state, context, predicted);
    }
}

/// [`PmoveTouch`] adapter for one movement tick.
pub struct PlayerTouch<'a, M: BrushModels + ?Sized> {
    entities: &'a mut MapEntities,
    models: &'a M,
    time_ms: i64,
}

impl<M: BrushModels + ?Sized> PmoveTouch for PlayerTouch<'_, M> {
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
