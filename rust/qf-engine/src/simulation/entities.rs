//! Map entities that react to the player: jump pads, teleporters and hurt
//! volumes.
//!
//! Behaviour follows the legacy game module (`g_trigger.cpp`, `g_misc.cpp`,
//! `g_target.cpp`, `client.cpp:G_TeleportPlayer`, `g_clip.cpp:G_PMoveTouchTriggers`
//! and `gs_misc.cpp:GS_TouchPushTrigger`). The system is independent of
//! rendering and windowing and runs from the fixed-tick movement through
//! [`PmoveTouch`], so a server and client prediction can share it.
//!
//! Entity storage is a plain `Vec` behind [`MapEntities`]; callers only see
//! the interface here, which keeps a later `bevy_ecs` backing an internal
//! change.
//!
//! Known differences from the legacy game:
//! - `G_PickTarget` chooses randomly among same-named targets; this picks the
//!   first, so the simulation stays deterministic.
//! - A jump pad whose target is not above it (NaN or zero airtime) is removed.
//! - There is no `use`/target firing, health, team or race-gametype logic:
//!   hurt volumes report damage as events, `START_OFF` volumes stay off.
//! - Telefragging (`KillBox`) is not applied; only the local player exists.

use std::collections::BTreeMap;

use crate::content::bsp::entities::Entity;

use super::pmove::{
    PMF_JUMPPAD_TIME, PMF_ON_GROUND, PMF_TIME_TELEPORT, PMF_WALLJUMPCOUNT, PmEvent, PmType,
    PmoveState, PmoveTouch, PredictedEvent, TouchContext, Vec3, angle_vectors,
};

/// Legacy `g_gravity` default, used when worldspawn has no `gravity` key.
pub const DEFAULT_LEVEL_GRAVITY: f32 = 850.0;
const MIN_TRIGGER_PUSH_REBOUNCE_TIME_MS: f32 = 100.0;
const SPAWNFLAG_PUSH_ONCE: i32 = 1;
const SPAWNFLAG_TELEPORT_SPECTATOR: i32 = 1;
const SPAWNFLAG_HURT_START_OFF: i32 = 1;
const SPAWNFLAG_HURT_NO_PROTECTION: i32 = 8;
const SPAWNFLAG_HURT_SLOW: i32 = 16;
const SPAWNFLAG_HURT_KILL: i32 = 32;
const SPAWNFLAG_HURT_FALL: i32 = 64;

/// Classnames that are turned into simulated entities.
pub const SUPPORTED_TRIGGERS: &[&str] = &["trigger_push", "trigger_teleport", "trigger_hurt"];
/// Classnames used only as aiming points or destinations of the triggers.
pub const SUPPORTED_TARGETS: &[&str] = &[
    "target_position",
    "info_notnull",
    "target_push",
    "misc_teleporter_dest",
    "info_teleport_destination",
];

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

#[derive(Clone, Debug, PartialEq)]
enum Behavior {
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
}

/// Where a teleporter sends the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Destination {
    pub origin: Vec3,
    /// Pitch (positive down), yaw, roll in the legacy convention.
    pub angles: Vec3,
}

#[derive(Clone, Debug, PartialEq)]
struct Trigger {
    entity: usize,
    classname: &'static str,
    model: usize,
    origin: Vec3,
    absmin: Vec3,
    absmax: Vec3,
    wait_ms: i64,
    /// `timeStamp`: the trigger can fire again once the level time passes it.
    ready_after: i64,
    enabled: bool,
    removed: bool,
    behavior: Behavior,
}

/// Something that happened because the player touched an entity.
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
}

/// Why an entity with a supported classname was not instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnWarning {
    pub entity: usize,
    pub classname: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Default)]
pub struct MapEntities {
    triggers: Vec<Trigger>,
    gravity: f32,
    warnings: Vec<SpawnWarning>,
    unsupported: BTreeMap<String, usize>,
    events: Vec<TriggerEvent>,
}

impl MapEntities {
    /// Instantiates supported entities from a parsed entity lump.
    pub fn spawn<M: BrushModels + ?Sized>(entities: &[Entity], models: &M) -> Self {
        let mut this = Self {
            gravity: DEFAULT_LEVEL_GRAVITY,
            ..Self::default()
        };
        if let Some(gravity) = entities
            .first()
            .filter(|entity| class_is(entity, "worldspawn"))
            .and_then(|entity| entity.get("gravity"))
        {
            this.gravity = atof(gravity);
        }

        for (index, entity) in entities.iter().enumerate() {
            let Some(classname) = entity.get("classname") else {
                continue;
            };
            let supported = SUPPORTED_TRIGGERS
                .iter()
                .find(|name| classname.eq_ignore_ascii_case(name));
            match supported {
                Some(&name) => this.spawn_trigger(index, name, entity, entities, models),
                None if SUPPORTED_TARGETS
                    .iter()
                    .any(|name| classname.eq_ignore_ascii_case(name))
                    || classname.eq_ignore_ascii_case("worldspawn") => {}
                None => {
                    *this
                        .unsupported
                        .entry(classname.to_ascii_lowercase())
                        .or_default() += 1;
                }
            }
        }
        this
    }

    pub fn level_gravity(&self) -> f32 {
        self.gravity
    }

    /// Entities with a supported-looking classname that could not be created.
    pub fn warnings(&self) -> &[SpawnWarning] {
        &self.warnings
    }

    /// Classnames present in the map that are not simulated, with counts.
    pub fn unsupported_classnames(&self) -> &BTreeMap<String, usize> {
        &self.unsupported
    }

    /// Number of live simulated triggers.
    pub fn trigger_count(&self) -> usize {
        self.triggers
            .iter()
            .filter(|trigger| !trigger.removed)
            .count()
    }

    /// Jump pad launch velocity of the `index`-th live trigger, when it is one.
    pub fn push_velocity(&self, entity: usize) -> Option<Vec3> {
        self.triggers
            .iter()
            .find_map(|trigger| match trigger.behavior {
                Behavior::Push { velocity, .. } if trigger.entity == entity && !trigger.removed => {
                    Some(velocity)
                }
                _ => None,
            })
    }

    /// Entity numbers (lump order) of live triggers with the given classname.
    pub fn triggers_of(&self, classname: &str) -> Vec<usize> {
        self.triggers
            .iter()
            .filter(|trigger| !trigger.removed && trigger.classname == classname)
            .map(|trigger| trigger.entity)
            .collect()
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&mut self) -> Vec<TriggerEvent> {
        std::mem::take(&mut self.events)
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

    fn spawn_trigger<M: BrushModels + ?Sized>(
        &mut self,
        index: usize,
        classname: &'static str,
        entity: &Entity,
        entities: &[Entity],
        models: &M,
    ) {
        let warn = |this: &mut Self, reason| {
            this.warnings.push(SpawnWarning {
                entity: index,
                classname: classname.to_owned(),
                reason,
            });
        };

        let Some(model) = entity
            .get("model")
            .and_then(|value| value.strip_prefix('*'))
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|&model| model != 0)
        else {
            return warn(self, "missing brush model");
        };
        let Some((mins, maxs)) = models.model_bounds(model) else {
            return warn(self, "brush model out of range");
        };
        let origin = entity
            .get("origin")
            .and_then(parse_vec3)
            .unwrap_or([0.0; 3]);
        let absmin = std::array::from_fn(|axis| origin[axis] + mins[axis] - 1.0);
        let absmax = std::array::from_fn(|axis| origin[axis] + maxs[axis] + 1.0);
        let spawnflags = entity.get("spawnflags").map_or(0, atoi);
        let wait = entity.get("wait").map_or(0.0, atof);

        let mut trigger = Trigger {
            entity: index,
            classname,
            model,
            origin,
            absmin,
            absmax,
            wait_ms: 0,
            ready_after: 0,
            enabled: true,
            removed: false,
            behavior: Behavior::Push {
                velocity: [0.0; 3],
                once: false,
            },
        };

        match classname {
            "trigger_push" => {
                let velocity = match entity.get("target") {
                    None => {
                        let direction = movedir(entity);
                        let speed = entity.get("speed").map_or(0.0, atof);
                        let speed = if speed != 0.0 { speed } else { 1000.0 };
                        direction.map(|component| component * (speed * 10.0))
                    }
                    Some(target) => {
                        let Some(target) = find_target(entities, target) else {
                            return warn(self, "jump pad target not found");
                        };
                        match self.jump_pad_velocity(&trigger, target.origin) {
                            Some(velocity) => velocity,
                            None => return warn(self, "jump pad target is not above the pad"),
                        }
                    }
                };
                trigger.behavior = Behavior::Push {
                    velocity,
                    once: spawnflags & SPAWNFLAG_PUSH_ONCE != 0,
                };
                let wait = if wait != 0.0 {
                    wait
                } else {
                    MIN_TRIGGER_PUSH_REBOUNCE_TIME_MS * 0.001
                };
                trigger.wait_ms = (wait * 1000.0) as i64;
            }
            "trigger_teleport" => {
                let Some(target) = entity.get("target") else {
                    return warn(self, "teleporter without a target");
                };
                let Some(destination) = find_target(entities, target) else {
                    return warn(self, "teleporter destination not found");
                };
                trigger.behavior = Behavior::Teleport {
                    destination: destination.as_destination(),
                    spectator_only: spawnflags & SPAWNFLAG_TELEPORT_SPECTATOR != 0,
                };
                trigger.wait_ms = (wait * 1000.0) as i64;
            }
            "trigger_hurt" => {
                let mut flags = spawnflags;
                let mut damage = entity.get("dmg").map_or(0, atoi);
                // Legacy hack: huge damage always kills.
                if damage > 300 {
                    flags |= SPAWNFLAG_HURT_KILL;
                }
                if damage == 0 {
                    damage = 5;
                }
                let wait = if flags & SPAWNFLAG_HURT_SLOW != 0 || wait == 0.0 {
                    0.1
                } else {
                    wait
                };
                trigger.wait_ms = (wait * 1000.0) as i64;
                trigger.enabled = flags & SPAWNFLAG_HURT_START_OFF == 0;
                trigger.behavior = Behavior::Hurt {
                    damage,
                    kill: flags & (SPAWNFLAG_HURT_KILL | SPAWNFLAG_HURT_FALL) != 0,
                    no_protection: flags & SPAWNFLAG_HURT_NO_PROTECTION != 0,
                };
            }
            _ => unreachable!("only supported trigger classnames are spawned"),
        }
        self.triggers.push(trigger);
    }

    /// `trigger_push_setup`: ballistic velocity from the pad centre to the target.
    fn jump_pad_velocity(&self, trigger: &Trigger, target: Vec3) -> Option<Vec3> {
        let center: [f32; 3] =
            std::array::from_fn(|axis| (trigger.absmin[axis] + trigger.absmax[axis]) * 0.5);
        let height = target[2] - center[2];
        // The legacy arithmetic promotes to double through `0.5 * level.gravity`.
        let time = (f64::from(height) / (0.5 * f64::from(self.gravity))).sqrt() as f32;
        if time == 0.0 || time.is_nan() {
            return None;
        }

        let mut velocity = [target[0] - center[0], target[1] - center[1], 0.0];
        let distance = (velocity[0] * velocity[0] + velocity[1] * velocity[1]).sqrt();
        if distance > 0.0 {
            velocity[0] /= distance;
            velocity[1] /= distance;
        }
        let horizontal_speed = distance / time;
        velocity[0] *= horizontal_speed;
        velocity[1] *= horizontal_speed;
        velocity[2] = time * self.gravity;
        Some(velocity)
    }

    fn touch(
        &mut self,
        models: &(impl BrushModels + ?Sized),
        time_ms: i64,
        state: &mut PmoveState,
        context: &TouchContext,
        predicted: &mut Vec<PredictedEvent>,
    ) {
        let (mins, maxs) = swept_bounds(state, context.previous_origin);
        // The candidate list is built before any touch runs, as in the legacy game.
        let candidates: Vec<usize> = self
            .triggers
            .iter()
            .enumerate()
            .filter(|(_, trigger)| {
                trigger.enabled
                    && !trigger.removed
                    && (0..3).all(|axis| {
                        mins[axis] <= trigger.absmax[axis] && maxs[axis] >= trigger.absmin[axis]
                    })
                    && models.box_touches_model(trigger.model, trigger.origin, mins, maxs)
            })
            .map(|(slot, _)| slot)
            .collect();

        for slot in candidates {
            if self.triggers[slot].removed {
                continue;
            }
            let trigger = &mut self.triggers[slot];
            let entity = trigger.entity;
            match trigger.behavior.clone() {
                Behavior::Push { velocity, once } => {
                    if !wait_elapsed(trigger, time_ms) {
                        continue;
                    }
                    // Spectators do not use jump pads (the wait is still consumed).
                    if state.pm_type == PmType::Normal {
                        state.velocity = velocity;
                        state.flags &= !(PMF_WALLJUMPCOUNT | PMF_ON_GROUND);
                        state.flags |= PMF_JUMPPAD_TIME;
                        predicted.push(PredictedEvent {
                            event: PmEvent::JumpPad,
                            parameter: 0,
                        });
                        self.events.push(TriggerEvent::JumpPad { entity, velocity });
                    }
                    if once {
                        self.triggers[slot].removed = true;
                    }
                }
                Behavior::Teleport {
                    destination,
                    spectator_only,
                } => {
                    if state.pm_type > PmType::Spectator {
                        continue;
                    }
                    if spectator_only && state.pm_type != PmType::Spectator {
                        continue;
                    }
                    if trigger.ready_after > time_ms {
                        continue;
                    }
                    trigger.ready_after = time_ms + trigger.wait_ms;
                    let velocity = teleport_player(state, context, &destination);
                    self.events.push(TriggerEvent::Teleport {
                        entity,
                        destination,
                        velocity,
                    });
                }
                Behavior::Hurt {
                    damage,
                    kill,
                    no_protection,
                } => {
                    if !wait_elapsed(trigger, time_ms) {
                        continue;
                    }
                    self.events.push(TriggerEvent::Hurt {
                        entity,
                        damage,
                        kill,
                        no_protection,
                    });
                }
            }
        }
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

/// `G_TriggerWait` (non-race): true when the trigger may fire now.
fn wait_elapsed(trigger: &mut Trigger, time_ms: i64) -> bool {
    if trigger.ready_after >= time_ms {
        return false;
    }
    trigger.ready_after = time_ms + trigger.wait_ms;
    true
}

/// `G_TeleportPlayer` for the movement state; returns the new velocity.
fn teleport_player(
    state: &mut PmoveState,
    context: &TouchContext,
    destination: &Destination,
) -> Vec3 {
    // Racesow behaviour: only the horizontal speed before the move is kept.
    let previous = context.previous_velocity;
    let speed = (previous[0] * previous[0] + previous[1] * previous[1]).sqrt();
    let (forward, _, _) = angle_vectors(destination.angles);
    state.velocity = forward.map(|component| component * speed);
    state.view_angles = destination.angles;
    state.origin = destination.origin;
    state.flags |= PMF_TIME_TELEPORT;
    state.pm_time = 1;
    state.velocity
}

/// Search box of `G_PMoveTouchTriggers`, covering the path since `previous`.
fn swept_bounds(state: &PmoveState, previous: Vec3) -> (Vec3, Vec3) {
    let mut mins = [0.0; 3];
    let mut maxs = [0.0; 3];
    for axis in 0..3 {
        let current = state.origin[axis];
        if previous[axis] < current {
            mins[axis] = previous[axis] + state.maxs[axis];
            if mins[axis] > current + state.mins[axis] {
                mins[axis] = current + state.mins[axis];
            }
            maxs[axis] = current + state.maxs[axis];
        } else {
            mins[axis] = current + state.mins[axis];
            maxs[axis] = previous[axis] + state.mins[axis];
            if maxs[axis] < current + state.maxs[axis] {
                maxs[axis] = current + state.maxs[axis];
            }
        }
    }
    (mins, maxs)
}

struct Target {
    origin: Vec3,
    angles: Vec3,
    classname: String,
}

impl Target {
    fn as_destination(&self) -> Destination {
        let mut origin = self.origin;
        // info_teleport_destination is spawned 16 units lower than where it is placed.
        if self
            .classname
            .eq_ignore_ascii_case("info_teleport_destination")
        {
            origin[2] += 16.0;
        }
        Destination {
            origin,
            angles: self.angles,
        }
    }
}

/// First entity with a matching `targetname` (`G_Find`).
fn find_target(entities: &[Entity], targetname: &str) -> Option<Target> {
    entities
        .iter()
        .find(|entity| {
            entity
                .get("targetname")
                .is_some_and(|name| name.eq_ignore_ascii_case(targetname))
                && !["info_null", "info_camp"]
                    .iter()
                    .any(|freed| class_is(entity, freed))
        })
        .map(|entity| Target {
            origin: entity
                .get("origin")
                .and_then(parse_vec3)
                .unwrap_or([0.0; 3]),
            angles: entity_angles(entity),
            classname: entity.get("classname").unwrap_or_default().to_owned(),
        })
}

fn class_is(entity: &Entity, classname: &str) -> bool {
    entity
        .get("classname")
        .is_some_and(|value| value.eq_ignore_ascii_case(classname))
}

/// `angles`, or the `angle` hack that only sets yaw.
fn entity_angles(entity: &Entity) -> Vec3 {
    if let Some(angles) = entity.get("angles").and_then(parse_vec3) {
        return angles;
    }
    [0.0, entity.get("angle").map_or(0.0, atof), 0.0]
}

/// `G_SetMovedir`: yaw -1/-2 mean straight up/down.
fn movedir(entity: &Entity) -> Vec3 {
    let angles = entity_angles(entity);
    if angles == [0.0, -1.0, 0.0] {
        [0.0, 0.0, 1.0]
    } else if angles == [0.0, -2.0, 0.0] {
        [0.0, 0.0, -1.0]
    } else {
        angle_vectors(angles).0
    }
}

fn parse_vec3(value: &str) -> Option<Vec3> {
    let values: Vec<f32> = value
        .split_ascii_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let vector: Vec3 = values.try_into().ok()?;
    vector.iter().all(|v| v.is_finite()).then_some(vector)
}

/// `atof`: parses the longest numeric prefix, 0 when there is none.
fn atof(value: &str) -> f32 {
    let value = value.trim_start();
    (1..=value.len())
        .rev()
        .filter(|&end| value.is_char_boundary(end))
        .find_map(|end| value[..end].parse::<f32>().ok().filter(|v| v.is_finite()))
        .unwrap_or(0.0)
}

fn atoi(value: &str) -> i32 {
    atof(value) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::pmove::{
        PmTrace, PmoveCommand, PmoveRules, PmoveWorld, pmove_with_touch,
    };

    /// Axis-aligned boxes standing in for inline brush models.
    struct Boxes(Vec<(Vec3, Vec3)>);

    impl BrushModels for Boxes {
        fn model_bounds(&self, index: usize) -> Option<(Vec3, Vec3)> {
            self.0.get(index).copied()
        }

        fn box_touches_model(&self, index: usize, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
            let (lo, hi) = self.0[index];
            (0..3).all(|axis| {
                mins[axis] <= hi[axis] + origin[axis] && maxs[axis] >= lo[axis] + origin[axis]
            })
        }
    }

    struct EmptyWorld;

    impl PmoveWorld for EmptyWorld {
        fn trace(&self, _: Vec3, _: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
            PmTrace::miss(end)
        }

        fn point_contents(&self, _: Vec3) -> u32 {
            0
        }
    }

    fn ent(properties: &[(&str, &str)]) -> Entity {
        Entity {
            properties: properties
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn step(
        entities: &mut MapEntities,
        boxes: &Boxes,
        state: &mut PmoveState,
        time_ms: i64,
    ) -> Vec<PredictedEvent> {
        let command = PmoveCommand {
            msec: 8,
            ..PmoveCommand::default()
        };
        let mut touch = entities.player_touch(boxes, time_ms);
        pmove_with_touch(
            &EmptyWorld,
            &PmoveRules::default(),
            state,
            &command,
            &mut touch,
        )
        .events
    }

    #[test]
    fn jump_pad_velocity_matches_legacy_formula() {
        // Pad centre (0,0,0), target 300 units east and 128 up, gravity 800.
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-32.0, -32.0, -8.0], [32.0, 32.0, 8.0]),
        ]);
        let entities = [
            ent(&[("classname", "worldspawn"), ("gravity", "800")]),
            ent(&[
                ("classname", "trigger_push"),
                ("model", "*1"),
                ("target", "t1"),
            ]),
            ent(&[
                ("classname", "target_position"),
                ("targetname", "t1"),
                ("origin", "300 0 128"),
            ]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        let velocity = map.push_velocity(1).unwrap();
        let time = (128.0f64 / 400.0).sqrt() as f32;
        assert!((velocity[0] - 300.0 / time).abs() < 1e-3);
        assert_eq!(velocity[1], 0.0);
        assert!((velocity[2] - time * 800.0).abs() < 1e-3);
        // Ballistic check: apex height at that airtime equals the target height.
        assert!((velocity[2] * time - 0.5 * 800.0 * time * time - 128.0).abs() < 0.01);
    }

    #[test]
    fn jump_pad_without_target_uses_angle_and_speed() {
        let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
        let entities = [
            ent(&[
                ("classname", "trigger_push"),
                ("model", "*1"),
                ("angle", "-1"),
            ]),
            ent(&[
                ("classname", "trigger_push"),
                ("model", "*1"),
                ("angle", "90"),
                ("speed", "50"),
            ]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        assert_eq!(map.push_velocity(0), Some([0.0, 0.0, 10_000.0]));
        let side = map.push_velocity(1).unwrap();
        assert!(side[0].abs() < 1e-3 && (side[1] - 500.0).abs() < 1e-3);
    }

    #[test]
    fn jump_pad_not_below_target_is_removed_with_warning() {
        let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
        let entities = [
            ent(&[
                ("classname", "trigger_push"),
                ("model", "*1"),
                ("target", "t"),
            ]),
            ent(&[
                ("classname", "info_notnull"),
                ("targetname", "t"),
                ("origin", "9 0 0"),
            ]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        assert_eq!(map.trigger_count(), 0);
        assert_eq!(map.warnings().len(), 1);
    }

    #[test]
    fn player_on_jump_pad_is_launched_deterministically() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [
            ent(&[("classname", "worldspawn"), ("gravity", "800")]),
            ent(&[
                ("classname", "trigger_push"),
                ("model", "*1"),
                ("target", "t1"),
            ]),
            ent(&[
                ("classname", "target_position"),
                ("targetname", "t1"),
                ("origin", "300 0 128"),
            ]),
        ];
        let expected = MapEntities::spawn(&entities, &boxes)
            .push_velocity(1)
            .unwrap();
        let run = || {
            let mut map = MapEntities::spawn(&entities, &boxes);
            let mut state = PmoveState::new([0.0, 0.0, 24.0]);
            state.flags |= PMF_ON_GROUND;
            let events = step(&mut map, &boxes, &mut state, 8);
            (state, events, map.drain_events())
        };
        let (state, events, triggered) = run();
        assert_eq!(state.velocity, expected);
        assert!(state.flags & PMF_ON_GROUND == 0);
        assert!(events.iter().any(|e| e.event == PmEvent::JumpPad));
        assert_eq!(
            triggered,
            vec![TriggerEvent::JumpPad {
                entity: 1,
                velocity: expected
            }]
        );
        let (again, _, _) = run();
        assert_eq!(again, state);
    }

    #[test]
    fn jump_pad_rearms_after_wait_and_ignores_spectators() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("angle", "-1"),
        ])];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, 8);
        assert_eq!(map.drain_events().len(), 1);
        // Within the 100 ms default wait: no second push.
        for tick in 2..=13 {
            state = PmoveState::new([0.0, 0.0, 24.0]);
            step(&mut map, &boxes, &mut state, tick * 8);
        }
        assert!(map.drain_events().is_empty());
        state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, 14 * 8);
        assert_eq!(map.drain_events().len(), 1);

        let mut spectator = PmoveState::new([0.0, 0.0, 24.0]);
        spectator.pm_type = PmType::Spectator;
        step(&mut map, &boxes, &mut spectator, 40 * 8);
        assert!(map.drain_events().is_empty());
        assert_eq!(spectator.velocity, [0.0; 3]);
    }

    #[test]
    fn push_once_removes_itself() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [ent(&[
            ("classname", "trigger_push"),
            ("model", "*1"),
            ("angle", "-1"),
            ("spawnflags", "1"),
        ])];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, 8);
        assert_eq!(map.trigger_count(), 0);
    }

    #[test]
    fn teleporter_moves_player_keeping_horizontal_speed_along_destination() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [
            ent(&[
                ("classname", "trigger_teleport"),
                ("model", "*1"),
                ("target", "d"),
            ]),
            ent(&[
                ("classname", "misc_teleporter_dest"),
                ("targetname", "d"),
                ("origin", "1000 2000 3000"),
                ("angle", "90"),
            ]),
        ];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        state.velocity = [300.0, 400.0, -700.0];
        let events = step(&mut map, &boxes, &mut state, 8);
        assert!(events.is_empty());
        assert_eq!(state.origin, [1000.0, 2000.0, 3000.0]);
        // 500 ups horizontal speed before the move, aimed along yaw 90.
        assert!(state.velocity[0].abs() < 1e-2);
        assert!((state.velocity[1] - 500.0).abs() < 1e-2);
        assert_eq!(state.velocity[2], 0.0);
        assert_eq!(state.view_angles, [0.0, 90.0, 0.0]);
        assert!(state.flags & PMF_TIME_TELEPORT != 0);
        assert_eq!(state.pm_time, 1);
        // Delta angles make the next command's view equal the destination view.
        assert_eq!(state.delta_angles, [0, 16384, 0]);
        assert!(matches!(
            map.drain_events().as_slice(),
            [TriggerEvent::Teleport { entity: 0, .. }]
        ));
    }

    #[test]
    fn teleport_destination_info_teleport_destination_is_raised_16() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [
            ent(&[
                ("classname", "trigger_teleport"),
                ("model", "*1"),
                ("target", "d"),
            ]),
            ent(&[
                ("classname", "info_teleport_destination"),
                ("targetname", "d"),
                ("origin", "0 0 100"),
            ]),
        ];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, 8);
        assert_eq!(state.origin, [0.0, 0.0, 116.0]);
    }

    #[test]
    fn teleporter_without_valid_target_is_not_spawned() {
        let boxes = Boxes(vec![([0.0; 3], [0.0; 3]), ([-8.0; 3], [8.0; 3])]);
        let entities = [
            ent(&[("classname", "trigger_teleport"), ("model", "*1")]),
            ent(&[
                ("classname", "trigger_teleport"),
                ("model", "*1"),
                ("target", "none"),
            ]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        assert_eq!(map.trigger_count(), 0);
        assert_eq!(map.warnings().len(), 2);
    }

    #[test]
    fn hurt_volume_reports_damage_every_wait_interval() {
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([-64.0, -64.0, -8.0], [64.0, 64.0, 8.0]),
        ]);
        let entities = [
            ent(&[
                ("classname", "trigger_hurt"),
                ("model", "*1"),
                ("dmg", "10"),
            ]),
            ent(&[
                ("classname", "trigger_hurt"),
                ("model", "*1"),
                ("dmg", "500"),
            ]),
        ];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 24.0]);
        step(&mut map, &boxes, &mut state, 8);
        assert_eq!(
            map.drain_events(),
            vec![
                TriggerEvent::Hurt {
                    entity: 0,
                    damage: 10,
                    kill: false,
                    no_protection: false
                },
                TriggerEvent::Hurt {
                    entity: 1,
                    damage: 500,
                    kill: true,
                    no_protection: false
                },
            ]
        );
        // 100 ms wait: silent until time passes 8 + 100.
        step(&mut map, &boxes, &mut state, 16);
        assert!(map.drain_events().is_empty());
        step(&mut map, &boxes, &mut state, 116);
        assert_eq!(map.drain_events().len(), 2);
    }

    #[test]
    fn swept_box_catches_fast_movement_through_a_thin_trigger() {
        // Legacy swept bounds: a trigger crossed within one tick is still touched.
        let boxes = Boxes(vec![
            ([0.0; 3], [0.0; 3]),
            ([90.0, -64.0, -64.0], [91.0, 64.0, 64.0]),
        ]);
        let entities = [ent(&[("classname", "trigger_hurt"), ("model", "*1")])];
        let mut map = MapEntities::spawn(&entities, &boxes);
        let mut state = PmoveState::new([0.0, 0.0, 0.0]);
        state.origin = [200.0, 0.0, 0.0];
        let context = TouchContext {
            previous_origin: [0.0, 0.0, 0.0],
            previous_velocity: [0.0; 3],
            command_angles: [0; 3],
        };
        let mut events = Vec::new();
        map.touch(&boxes, 8, &mut state, &context, &mut events);
        assert_eq!(map.drain_events().len(), 1);
    }

    #[test]
    fn unsupported_classnames_are_counted() {
        let boxes = Boxes(vec![([0.0; 3], [0.0; 3])]);
        let entities = [
            ent(&[("classname", "worldspawn")]),
            ent(&[("classname", "item_health_small")]),
            ent(&[("classname", "ITEM_HEALTH_SMALL")]),
            ent(&[("classname", "target_position")]),
        ];
        let map = MapEntities::spawn(&entities, &boxes);
        assert_eq!(
            map.unsupported_classnames().get("item_health_small"),
            Some(&2)
        );
        assert_eq!(map.unsupported_classnames().len(), 1);
    }

    #[test]
    fn atof_parses_prefixes() {
        assert_eq!(atof("0.2"), 0.2);
        assert_eq!(atof("-1"), -1.0);
        assert_eq!(atof("12abc"), 12.0);
        assert_eq!(atof("abc"), 0.0);
    }
}
