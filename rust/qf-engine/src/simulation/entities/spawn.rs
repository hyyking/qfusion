//! Instantiates the entity lump into a `bevy_ecs` world.
//!
//! Every lump entity becomes an ECS entity carrying its keys; supported
//! classnames additionally get behaviour components. Entities the legacy game
//! frees at spawn (`info_null`, `info_camp`, or triggers with unusable keys)
//! are not instantiated.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;

use crate::content::bsp::entities::Entity as LumpEntity;
use crate::simulation::pmove::Vec3;

use super::components::{
    Classname, Enabled, Level, LumpIndex, Occupants, Outbox, PendingTouches, Placement, Player,
    Registry, Think, ThinkKind, Timing, TouchKind, UseKind, Volume,
};
use super::parse::{
    atof, atoi, class_is, freed_at_spawn, links_of, movedir, passes_gametype_filter, placement_of,
    seconds_to_ms,
};
use super::{BrushModels, DEFAULT_LEVEL_GRAVITY, Destination, Gametype, SpawnWarning};

const MIN_TRIGGER_PUSH_REBOUNCE_TIME_S: f32 = 0.1;
const MIN_TRIGGER_ALWAYS_DELAY_S: f32 = 0.3;
const SPAWNFLAG_PUSH_ONCE: i32 = 1;
const SPAWNFLAG_TELEPORT_SPECTATOR: i32 = 1;
const SPAWNFLAG_HURT_START_OFF: i32 = 1;
const SPAWNFLAG_HURT_TOGGLE: i32 = 2;
const SPAWNFLAG_HURT_NO_PROTECTION: i32 = 8;
const SPAWNFLAG_HURT_SLOW: i32 = 16;
const SPAWNFLAG_HURT_KILL: i32 = 32;
const SPAWNFLAG_HURT_FALL: i32 = 64;
const SPAWNFLAG_MULTIPLE_NOT_PLAYER: i32 = 2;
const SPAWNFLAG_MULTIPLE_TRIGGERED: i32 = 4;
const SPAWNFLAG_COUNTER_NOMESSAGE: i32 = 1;
const SPAWNFLAG_TELEPORTER_SPECTATOR: i32 = 1;
const SPAWNFLAG_PRINT_CLEAR: i32 = 8;
const DEFAULT_COUNTER_COUNT: i32 = 2;
const DEFAULT_MULTIPLE_WAIT_S: f32 = 0.2;

pub(super) struct Spawned {
    pub world: World,
    pub warnings: Vec<SpawnWarning>,
    pub unsupported: BTreeMap<String, usize>,
}

pub(super) fn spawn_all<M: BrushModels + ?Sized>(
    lump: &[LumpEntity],
    models: &M,
    gametype: &Gametype,
) -> Spawned {
    let gravity = lump
        .first()
        .filter(|entity| class_is(entity, "worldspawn"))
        .and_then(|entity| entity.get("gravity"))
        .map_or(DEFAULT_LEVEL_GRAVITY, atof);

    let mut world = World::new();
    world.insert_resource(Level {
        time_ms: 0,
        gravity,
    });
    world.init_resource::<Player>();
    world.init_resource::<Occupants>();
    world.init_resource::<PendingTouches>();
    world.init_resource::<Outbox>();

    let alive: Vec<bool> = lump
        .iter()
        .map(|entity| {
            entity.get("classname").is_some()
                && !freed_at_spawn(entity)
                && passes_gametype_filter(entity, gametype)
        })
        .collect();

    // Pass 1: every entity exists before any behaviour looks up its targets.
    let mut by_lump = Vec::with_capacity(lump.len());
    for (index, entity) in lump.iter().enumerate() {
        let spawned = match entity.get("classname") {
            Some(classname) if alive[index] => Some(
                world
                    .spawn((
                        LumpIndex(index),
                        Classname(classname.to_ascii_lowercase()),
                        links_of(entity),
                        placement_of(entity),
                    ))
                    .id(),
            ),
            _ => None,
        };
        by_lump.push(spawned);
    }
    world.insert_resource(Registry { by_lump });

    // Pass 2: behaviours.
    let mut warnings = Vec::new();
    let mut unsupported = BTreeMap::new();
    for (index, entity) in lump.iter().enumerate() {
        let Some(ecs) = world.resource::<Registry>().by_lump[index] else {
            continue;
        };
        let classname = entity
            .get("classname")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut spawner = Spawner {
            world: &mut world,
            lump,
            alive: &alive,
            models,
            gravity,
            ecs,
            entity,
        };
        match spawner.spawn_behavior(&classname) {
            Ok(true) => {}
            Ok(false) => *unsupported.entry(classname).or_default() += 1,
            Err(reason) => {
                world.despawn(ecs);
                warnings.push(SpawnWarning {
                    entity: index,
                    classname,
                    reason,
                });
            }
        }
    }

    Spawned {
        world,
        warnings,
        unsupported,
    }
}

struct Spawner<'a, M: BrushModels + ?Sized> {
    world: &'a mut World,
    lump: &'a [LumpEntity],
    alive: &'a [bool],
    models: &'a M,
    gravity: f32,
    ecs: Entity,
    entity: &'a LumpEntity,
}

impl<M: BrushModels + ?Sized> Spawner<'_, M> {
    /// `Ok(false)` means the classname has no simulated behaviour.
    fn spawn_behavior(&mut self, classname: &str) -> Result<bool, &'static str> {
        let spawnflags = self.entity.get("spawnflags").map_or(0, atoi);
        let wait = self.entity.get("wait").map_or(0.0, atof);
        match classname {
            "trigger_push" => self.trigger_push(spawnflags, wait)?,
            "trigger_teleport" => self.trigger_teleport(spawnflags, wait)?,
            "trigger_hurt" => self.trigger_hurt(spawnflags, wait)?,
            "trigger_multiple" | "trigger_once" => {
                self.trigger_multiple(classname == "trigger_once", spawnflags, wait)?
            }
            "trigger_counter" => self.trigger_counter(spawnflags),
            "trigger_always" => self.trigger_always(),
            "trigger_relay" | "target_relay" => self.insert(UseKind::Relay),
            "target_delay" => self.target_delay(wait),
            "target_kill" => self.insert(UseKind::Kill),
            "target_print" => self.insert(UseKind::Print {
                silent: spawnflags & SPAWNFLAG_PRINT_CLEAR != 0,
            }),
            "target_teleporter" => self.insert(UseKind::Teleporter {
                spectator_only: spawnflags & SPAWNFLAG_TELEPORTER_SPECTATOR != 0,
            }),
            "func_killbox" => {
                let volume = self.volume()?;
                self.insert((volume, UseKind::Killbox));
            }
            // Aiming points, destinations and entities with no effect on the player:
            // `trigger_gravity` only sets `edict_t::gravity`, which pmove never reads.
            _ if super::is_inert(classname) => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn insert(&mut self, bundle: impl Bundle) {
        self.world.entity_mut(self.ecs).insert(bundle);
    }

    /// `GClip_SetBrushModel`: bounds of the inline model named by `model`.
    fn volume(&self) -> Result<Volume, &'static str> {
        let model = self
            .entity
            .get("model")
            .and_then(|value| value.strip_prefix('*'))
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|&model| model != 0)
            .ok_or("missing brush model")?;
        let (mins, maxs) = self
            .models
            .model_bounds(model)
            .ok_or("brush model out of range")?;
        let origin = placement_of(self.entity).origin;
        Ok(Volume {
            model,
            origin,
            mins,
            maxs,
            absmin: std::array::from_fn(|axis| origin[axis] + mins[axis] - 1.0),
            absmax: std::array::from_fn(|axis| origin[axis] + maxs[axis] + 1.0),
        })
    }

    /// First entity with a matching `targetname` (`G_Find`).
    fn find_target(&self, targetname: &str) -> Option<Placement> {
        self.lump
            .iter()
            .zip(self.alive)
            .find(|(entity, alive)| {
                **alive
                    && entity
                        .get("targetname")
                        .is_some_and(|name| name.eq_ignore_ascii_case(targetname))
            })
            .map(|(entity, _)| placement_of(entity))
    }

    fn trigger_push(&mut self, spawnflags: i32, wait: f32) -> Result<(), &'static str> {
        let volume = self.volume()?;
        let velocity = match self.entity.get("target") {
            None => {
                let speed = self.entity.get("speed").map_or(0.0, atof);
                let speed = if speed != 0.0 { speed } else { 1000.0 };
                movedir(self.entity).map(|component| component * (speed * 10.0))
            }
            Some(target) => {
                let target = self
                    .find_target(target)
                    .ok_or("jump pad target not found")?;
                jump_pad_velocity(&volume, target.origin, self.gravity)
                    .ok_or("jump pad target is not above the pad")?
            }
        };
        let wait_s = if wait != 0.0 {
            wait
        } else {
            MIN_TRIGGER_PUSH_REBOUNCE_TIME_S
        };
        self.insert((
            volume,
            Timing {
                wait_s,
                ready_after: 0,
            },
            Enabled(true),
            TouchKind::Push {
                velocity,
                once: spawnflags & SPAWNFLAG_PUSH_ONCE != 0,
            },
        ));
        Ok(())
    }

    fn trigger_teleport(&mut self, spawnflags: i32, wait: f32) -> Result<(), &'static str> {
        let target = self
            .entity
            .get("target")
            .ok_or("teleporter without a target")?;
        let destination = self
            .find_target(target)
            .ok_or("teleporter destination not found")?;
        let volume = self.volume()?;
        self.insert((
            volume,
            Timing {
                wait_s: wait,
                ready_after: 0,
            },
            Enabled(true),
            TouchKind::Teleport {
                destination: Destination {
                    origin: destination.origin,
                    angles: destination.angles,
                },
                spectator_only: spawnflags & SPAWNFLAG_TELEPORT_SPECTATOR != 0,
            },
        ));
        Ok(())
    }

    fn trigger_hurt(&mut self, spawnflags: i32, wait: f32) -> Result<(), &'static str> {
        let volume = self.volume()?;
        let mut flags = spawnflags;
        let mut damage = self.entity.get("dmg").map_or(0, atoi);
        // Legacy hack: huge damage always kills.
        if damage > 300 {
            flags |= SPAWNFLAG_HURT_KILL;
        }
        if damage == 0 {
            damage = 5;
        }
        let wait_s = if flags & SPAWNFLAG_HURT_SLOW != 0 || wait == 0.0 {
            0.1
        } else {
            wait
        };
        self.insert((
            volume,
            Timing {
                wait_s,
                ready_after: 0,
            },
            Enabled(flags & SPAWNFLAG_HURT_START_OFF == 0),
            TouchKind::Hurt {
                damage,
                kill: flags & (SPAWNFLAG_HURT_KILL | SPAWNFLAG_HURT_FALL) != 0,
                no_protection: flags & SPAWNFLAG_HURT_NO_PROTECTION != 0,
            },
        ));
        if flags & SPAWNFLAG_HURT_TOGGLE != 0 {
            self.insert(UseKind::HurtToggle);
        }
        Ok(())
    }

    fn trigger_multiple(
        &mut self,
        once: bool,
        spawnflags: i32,
        wait: f32,
    ) -> Result<(), &'static str> {
        let volume = self.volume()?;
        let wait_s = if once {
            -1.0
        } else if wait == 0.0 {
            DEFAULT_MULTIPLE_WAIT_S
        } else {
            wait
        };
        let triggered = spawnflags & SPAWNFLAG_MULTIPLE_TRIGGERED != 0;
        self.insert((
            volume,
            Timing {
                wait_s,
                ready_after: 0,
            },
            Enabled(!triggered),
            TouchKind::Multi {
                not_player: spawnflags & SPAWNFLAG_MULTIPLE_NOT_PLAYER != 0,
            },
            if triggered {
                UseKind::Enable
            } else {
                UseKind::Multi
            },
        ));
        Ok(())
    }

    fn trigger_counter(&mut self, spawnflags: i32) {
        let count = self.entity.get("count").map_or(0, atoi);
        self.insert((
            Timing {
                wait_s: -1.0,
                ready_after: 0,
            },
            UseKind::Counter {
                remaining: if count != 0 {
                    count
                } else {
                    DEFAULT_COUNTER_COUNT
                },
                silent: spawnflags & SPAWNFLAG_COUNTER_NOMESSAGE != 0,
            },
        ));
    }

    /// The think re-enters `G_UseTargets`, which applies `delay` a second time as in
    /// the legacy game.
    fn trigger_always(&mut self) {
        let delay = self
            .entity
            .get("delay")
            .map_or(0.0, atof)
            .max(MIN_TRIGGER_ALWAYS_DELAY_S);
        let delay_ms = seconds_to_ms(delay);
        let mut links = links_of(self.entity);
        links.delay_ms = delay_ms;
        self.insert((
            links,
            Think {
                due_ms: delay_ms,
                kind: ThinkKind::Always,
            },
        ));
    }

    fn target_delay(&mut self, wait: f32) {
        // The `delay` key is honoured for Q3 maps, then cleared.
        let delay = self.entity.get("delay").map_or(0.0, atof);
        let wait_s = match (delay, wait) {
            (delay, _) if delay != 0.0 => delay,
            (_, wait) if wait != 0.0 => wait,
            _ => 1.0,
        };
        let mut links = links_of(self.entity);
        links.delay_ms = 0;
        self.insert((links, UseKind::Delay { wait_s }));
    }
}

/// `trigger_push_setup`: ballistic velocity from the pad centre to the target.
fn jump_pad_velocity(pad: &Volume, target: Vec3, gravity: f32) -> Option<Vec3> {
    let center: Vec3 = std::array::from_fn(|axis| (pad.absmin[axis] + pad.absmax[axis]) * 0.5);
    let height = target[2] - center[2];
    // The legacy arithmetic promotes to double through `0.5 * level.gravity`.
    let time = (f64::from(height) / (0.5 * f64::from(gravity))).sqrt() as f32;
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
    velocity[2] = time * gravity;
    Some(velocity)
}
