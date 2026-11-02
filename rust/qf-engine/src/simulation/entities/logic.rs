//! Systems of the entity world: touch search, thinks, `G_UseTargets`, teleporting
//! and telefragging. All of it runs on plain `&mut World` so the order of
//! execution is explicit and deterministic.

use bevy_ecs::prelude::*;

use crate::simulation::pmove::{
    PMF_JUMPPAD_TIME, PMF_ON_GROUND, PMF_TIME_TELEPORT, PMF_WALLJUMPCOUNT, PmEvent, PmType,
    PmoveState, PredictedEvent, TouchContext, Vec3, angle_vectors,
};

use super::components::{
    Enabled, Level, Links, LumpIndex, Occupants, Outbox, PendingTouches, Placement, Player,
    PlayerPresence, Think, ThinkKind, Timing, TouchKind, UseKind, Volume,
};
use super::parse::seconds_to_ms;
use super::{BrushModels, Destination, TriggerEvent, Victim};

fn emit(world: &mut World, event: TriggerEvent) {
    world.resource_mut::<Outbox>().events.push(event);
}

fn now(world: &World) -> i64 {
    world.resource::<Level>().time_ms
}

fn lump_of(world: &World, entity: Entity) -> usize {
    world.get::<LumpIndex>(entity).map_or(usize::MAX, |l| l.0)
}

fn exists(world: &World, entity: Entity) -> bool {
    world.get_entity(entity).is_ok()
}

/// Live entities whose `targetname` matches, in lump order (`G_Find`).
fn find_by_targetname(world: &mut World, name: &str) -> Vec<Entity> {
    let mut query = world.query::<(Entity, &LumpIndex, &Links)>();
    let mut found: Vec<(usize, Entity)> = query
        .iter(world)
        .filter(|(_, _, links)| {
            links
                .targetname
                .as_deref()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
        })
        .map(|(entity, lump, _)| (lump.0, entity))
        .collect();
    found.sort_unstable();
    found.into_iter().map(|(_, entity)| entity).collect()
}

/// `G_TriggerWait` (non-race): true when the entity may fire now.
fn trigger_wait(world: &mut World, entity: Entity, time_ms: i64) -> bool {
    let Some(mut timing) = world.get_mut::<Timing>(entity) else {
        return true;
    };
    if timing.ready_after >= time_ms {
        return false;
    }
    timing.ready_after = time_ms + seconds_to_ms(timing.wait_s);
    true
}

/// `G_UseTargets`.
pub(super) fn use_targets(world: &mut World, source: Entity) {
    let Some(links) = world.get::<Links>(source).cloned() else {
        return;
    };
    let lump = lump_of(world, source);
    let time_ms = now(world);

    if links.delay_ms != 0 {
        world.spawn((
            LumpIndex(lump),
            Links {
                targetname: None,
                delay_ms: 0,
                ..links
            },
            Think {
                due_ms: time_ms + links.delay_ms,
                kind: ThinkKind::DelayedUse,
            },
        ));
        return;
    }

    if let Some(text) = links.message.filter(|text| !text.is_empty()) {
        emit(world, TriggerEvent::Message { entity: lump, text });
    }

    if let Some(name) = &links.killtarget {
        for victim in find_by_targetname(world, name) {
            world.despawn(victim);
        }
        if !exists(world, source) {
            return;
        }
    }

    if let Some(name) = &links.target {
        for target in find_by_targetname(world, name) {
            if target == source || world.get::<UseKind>(target).is_none() {
                continue;
            }
            let target_lump = lump_of(world, target);
            emit(
                world,
                TriggerEvent::Used {
                    source: lump,
                    target: target_lump,
                },
            );
            use_entity(world, target);
            if !exists(world, source) {
                return;
            }
        }
    }
}

/// `multi_trigger`.
fn multi_trigger(world: &mut World, entity: Entity) {
    let time_ms = now(world);
    if !trigger_wait(world, entity, time_ms) {
        return;
    }
    use_targets(world, entity);
    let wait_s = world.get::<Timing>(entity).map_or(0.0, |t| t.wait_s);
    if wait_s <= 0.0 {
        world.despawn(entity);
    }
}

/// Runs the entity's `use` callback.
pub(super) fn use_entity(world: &mut World, entity: Entity) {
    let Some(kind) = world.get::<UseKind>(entity).copied() else {
        return;
    };
    let lump = lump_of(world, entity);
    match kind {
        UseKind::Multi => multi_trigger(world, entity),
        UseKind::Mover(index) => super::movers::use_mover(world, index),
        UseKind::Enable => {
            if let Some(mut enabled) = world.get_mut::<Enabled>(entity) {
                enabled.0 = true;
            }
            world.entity_mut(entity).insert(UseKind::Multi);
        }
        UseKind::Relay => use_targets(world, entity),
        UseKind::Delay { wait_s } => {
            let due_ms = now(world) + seconds_to_ms(wait_s);
            world.entity_mut(entity).insert(Think {
                due_ms,
                kind: ThinkKind::Delay,
            });
        }
        UseKind::Kill => {
            if world.resource::<Player>().0.is_some() {
                emit(world, TriggerEvent::Kill { entity: lump });
            }
        }
        UseKind::Teleporter { spectator_only } => target_teleporter(world, entity, spectator_only),
        UseKind::Counter { remaining, silent } => {
            if remaining == 0 {
                return;
            }
            let remaining = remaining - 1;
            world
                .entity_mut(entity)
                .insert(UseKind::Counter { remaining, silent });
            if remaining != 0 {
                if !silent {
                    emit(
                        world,
                        TriggerEvent::Message {
                            entity: lump,
                            text: format!("{remaining} more to go..."),
                        },
                    );
                }
                return;
            }
            if !silent {
                emit(
                    world,
                    TriggerEvent::Message {
                        entity: lump,
                        text: "Sequence completed!".to_owned(),
                    },
                );
            }
            multi_trigger(world, entity);
        }
        UseKind::HurtToggle => {
            if let Some(mut enabled) = world.get_mut::<Enabled>(entity) {
                enabled.0 = !enabled.0;
            }
        }
        UseKind::Print { silent } => {
            let text = world.get::<Links>(entity).and_then(|l| l.message.clone());
            if let Some(text) = text.filter(|t| !silent && !t.is_empty()) {
                emit(world, TriggerEvent::Message { entity: lump, text });
            }
        }
        UseKind::Killbox => {
            let Some(volume) = world.get::<Volume>(entity).copied() else {
                return;
            };
            let mins = std::array::from_fn(|axis| volume.origin[axis] + volume.mins[axis]);
            let maxs = std::array::from_fn(|axis| volume.origin[axis] + volume.maxs[axis]);
            kill_box(world, lump, mins, maxs, true);
        }
    }
}

/// `target_teleporter_use`: sends the player to the first matching destination.
fn target_teleporter(world: &mut World, entity: Entity, spectator_only: bool) {
    let Some(player) = world.resource::<Player>().0 else {
        return;
    };
    if player.state.pm_type > PmType::Spectator {
        return;
    }
    if spectator_only && player.state.pm_type != PmType::Spectator {
        return;
    }
    let Some(target) = world
        .get::<Links>(entity)
        .and_then(|links| links.target.clone())
    else {
        return;
    };
    let Some(destination) = find_by_targetname(world, &target)
        .into_iter()
        .find_map(|found| world.get::<Placement>(found).copied())
    else {
        return;
    };
    let lump = lump_of(world, entity);
    teleport_local(
        world,
        lump,
        Destination {
            origin: destination.origin,
            angles: destination.angles,
        },
    );
}

/// `G_TeleportPlayer` for the local player, followed by `KillBox`.
fn teleport_local(world: &mut World, lump: usize, destination: Destination) {
    let Some(mut player) = world.resource::<Player>().0 else {
        return;
    };
    let velocity = teleport_player(&mut player.state, &player.context, &destination);
    let (mins, maxs) = absolute_box(&player.state);
    world.resource_mut::<Player>().0 = Some(player);
    emit(
        world,
        TriggerEvent::Teleport {
            entity: lump,
            destination,
            velocity,
        },
    );
    kill_box(world, lump, mins, maxs, false);
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

fn absolute_box(state: &PmoveState) -> (Vec3, Vec3) {
    (
        std::array::from_fn(|axis| state.origin[axis] + state.mins[axis]),
        std::array::from_fn(|axis| state.origin[axis] + state.maxs[axis]),
    )
}

fn boxes_overlap(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    (0..3).all(|axis| a.0[axis] < b.1[axis] && a.1[axis] > b.0[axis])
}

/// `KillBox`: every other solid player inside the box is telefragged. Touching
/// faces do not count. `include_player` also reports the local player.
fn kill_box(world: &mut World, source: usize, mins: Vec3, maxs: Vec3, include_player: bool) {
    let mut victims = Vec::new();
    if include_player
        && let Some(player) = world.resource::<Player>().0
        && boxes_overlap((mins, maxs), absolute_box(&player.state))
    {
        victims.push(Victim::Player);
    }
    victims.extend(
        world
            .resource::<Occupants>()
            .0
            .iter()
            .filter(|occupant| {
                let occupant_box = (
                    std::array::from_fn(|axis| occupant.origin[axis] + occupant.mins[axis]),
                    std::array::from_fn(|axis| occupant.origin[axis] + occupant.maxs[axis]),
                );
                boxes_overlap((mins, maxs), occupant_box)
            })
            .map(|occupant| Victim::Occupant(occupant.id)),
    );
    for victim in victims {
        emit(
            world,
            TriggerEvent::Telefrag {
                entity: source,
                victim,
            },
        );
    }
}

/// Runs due thinks in (time, lump, entity) order.
pub(super) fn run_thinks(world: &mut World) {
    let time_ms = now(world);
    let mut query = world.query::<(Entity, &Think, Option<&LumpIndex>)>();
    let mut due: Vec<(i64, usize, u64, Entity, ThinkKind)> = query
        .iter(world)
        .filter(|(_, think, _)| think.due_ms <= time_ms)
        .map(|(entity, think, lump)| {
            (
                think.due_ms,
                lump.map_or(usize::MAX, |l| l.0),
                entity.to_bits(),
                entity,
                think.kind,
            )
        })
        .collect();
    due.sort_unstable_by_key(|&(due_ms, lump, bits, ..)| (due_ms, lump, bits));

    for (.., entity, kind) in due {
        // A previous think may have freed the entity or rescheduled it.
        if world
            .get::<Think>(entity)
            .is_none_or(|think| think.due_ms > time_ms)
        {
            continue;
        }
        world.entity_mut(entity).remove::<Think>();
        use_targets(world, entity);
        if matches!(kind, ThinkKind::Always | ThinkKind::DelayedUse) && exists(world, entity) {
            world.despawn(entity);
        }
    }
}

/// `G_PMoveTouchTriggers`: touch search over the swept box, then the touches.
pub(super) fn touch(
    world: &mut World,
    models: &(impl BrushModels + ?Sized),
    time_ms: i64,
    state: &mut PmoveState,
    context: &TouchContext,
    predicted: &mut Vec<PredictedEvent>,
) {
    world.resource_mut::<Level>().set_time(time_ms);
    world.resource_mut::<Player>().0 = Some(PlayerPresence {
        state: *state,
        context: *context,
    });
    run_thinks(world);

    // The candidate list is built before any touch runs, as in the legacy game.
    let (mins, maxs) = swept_bounds(state, context.previous_origin);
    let mut query =
        world.query_filtered::<(Entity, &LumpIndex, &Volume, &Enabled), With<TouchKind>>();
    let mut candidates: Vec<(usize, Entity)> = query
        .iter(world)
        .filter(|(_, _, volume, enabled)| {
            enabled.0
                && (0..3).all(|axis| {
                    mins[axis] <= volume.absmax[axis] && maxs[axis] >= volume.absmin[axis]
                })
                // Model 0 marks a plain box volume (door and plat triggers).
                && (volume.model == 0
                    || models.box_touches_model(volume.model, volume.origin, mins, maxs))
        })
        .map(|(entity, lump, ..)| (lump.0, entity))
        .collect();
    candidates.sort_unstable();
    world.resource_mut::<PendingTouches>().0 = candidates.into_iter().map(|(_, e)| e).collect();

    for entity in std::mem::take(&mut world.resource_mut::<PendingTouches>().0) {
        touch_entity(world, entity, time_ms);
    }

    if let Some(player) = world.resource_mut::<Player>().0.take() {
        *state = player.state;
    }
    predicted.append(&mut world.resource_mut::<Outbox>().predicted);
}

fn touch_entity(world: &mut World, entity: Entity, time_ms: i64) {
    let Some(kind) = world.get::<TouchKind>(entity).copied() else {
        return;
    };
    let Some(mut player) = world.resource::<Player>().0 else {
        return;
    };
    let lump = lump_of(world, entity);
    match kind {
        TouchKind::Push { velocity, once } => {
            if !trigger_wait(world, entity, time_ms) {
                return;
            }
            // Spectators do not use jump pads (the wait is still consumed).
            if player.state.pm_type == PmType::Normal {
                player.state.velocity = velocity;
                player.state.flags &= !(PMF_WALLJUMPCOUNT | PMF_ON_GROUND);
                player.state.flags |= PMF_JUMPPAD_TIME;
                world.resource_mut::<Player>().0 = Some(player);
                world
                    .resource_mut::<Outbox>()
                    .predicted
                    .push(PredictedEvent {
                        event: PmEvent::JumpPad,
                        parameter: 0,
                    });
                emit(
                    world,
                    TriggerEvent::JumpPad {
                        entity: lump,
                        velocity,
                    },
                );
            }
            if once {
                world.despawn(entity);
            }
        }
        TouchKind::Teleport {
            destination,
            spectator_only,
        } => {
            if player.state.pm_type > PmType::Spectator
                || (spectator_only && player.state.pm_type != PmType::Spectator)
            {
                return;
            }
            let Some(mut timing) = world.get_mut::<Timing>(entity) else {
                return;
            };
            if timing.ready_after > time_ms {
                return;
            }
            timing.ready_after = time_ms + seconds_to_ms(timing.wait_s);
            teleport_local(world, lump, destination);
        }
        TouchKind::Hurt {
            damage,
            kill,
            no_protection,
        } => {
            if trigger_wait(world, entity, time_ms) {
                emit(
                    world,
                    TriggerEvent::Hurt {
                        entity: lump,
                        damage,
                        kill,
                        no_protection,
                    },
                );
            }
        }
        TouchKind::Multi { not_player } => {
            if !not_player {
                multi_trigger(world, entity);
            }
        }
        TouchKind::MoverTrigger { owner, plat } => {
            if player.state.pm_type == PmType::Normal {
                super::movers::trigger_touched(world, owner, plat);
            }
        }
    }
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
