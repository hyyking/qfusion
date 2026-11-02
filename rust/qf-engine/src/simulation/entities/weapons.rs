//! Weapons as components and a system of the fixed-tick schedule.
//!
//! Player entities carry an [`Inventory`], a [`WeaponCmd`] (the attack and weapon
//! select buttons of the usercmd) and an [`Alive`] flag. The `Weapons` system runs right
//! after `Movement`: it lends the rules of [`crate::game::firing`] a snapshot of every
//! player, applies the changed inventories and knockback back to the entities, keeps
//! projectiles as [`Shot`] entities and reports what happened through the outbox as
//! [`WeaponEvent`]s. Damage is only reported: whoever owns health applies it.

use bevy_ecs::prelude::*;

use crate::game::firing::{Combatant, PlayerInput, Projectile, WeaponEvent, Weapons};
use crate::game::inventory::Inventory;
use crate::simulation::player::TICK_DURATION;

use super::components::{Level, Outbox, SimRng};
use super::players::{Walker, in_slot_order};
use super::schedule::Borrowed;

/// Attack/select buttons of the usercmd; select and cycle are consumed by the tick.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WeaponCmd(pub PlayerInput);

/// Dead players keep their entity but neither shoot nor get hit.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Alive(pub bool);

/// A projectile in flight.
#[derive(Component, Clone, Debug, PartialEq)]
pub(super) struct Shot(pub Projectile);

/// Next projectile id; ids order projectiles within a tick.
#[derive(Resource, Default)]
pub(super) struct ShotIds(pub u32);

pub(super) fn install(world: &mut World) {
    world.init_resource::<ShotIds>();
}

/// Projectiles in id order.
pub(super) fn shots(world: &World) -> Vec<Projectile> {
    let Some(mut query) = world.try_query::<&Shot>() else {
        return Vec::new();
    };
    let mut shots: Vec<Projectile> = query.iter(world).map(|shot| shot.0.clone()).collect();
    shots.sort_unstable_by_key(|shot| shot.id);
    shots
}

pub(super) fn step(world: &mut World) {
    let players = in_slot_order(world);
    let existing = shots(world);
    if players.is_empty() && existing.is_empty() {
        return;
    }
    let tick = world.resource::<Level>().tick;
    let seed = world.resource::<SimRng>().seed;
    let next_id = world.resource::<ShotIds>().0;

    // Slots index the combatants directly; absent slots are dead placeholders.
    let size = players.last().map_or(0, |&(slot, _)| usize::from(slot) + 1);
    let mut combatants = vec![Combatant::default(); size];
    for &(slot, entity) in &players {
        let Some(walker) = world.get::<Walker>(entity) else {
            continue;
        };
        combatants[usize::from(slot)] = Combatant {
            alive: world.get::<Alive>(entity).is_none_or(|alive| alive.0),
            origin: walker.0.state.origin,
            yaw: walker.0.view_angles[1],
            pitch: -walker.0.view_angles[0],
            inventory: world.get::<Inventory>(entity).cloned().unwrap_or_default(),
            input: world
                .get::<WeaponCmd>(entity)
                .map_or_else(Default::default, |c| c.0),
            impulse: [0.0; 3],
        };
    }

    let mut weapons = Weapons::from_parts(existing, next_id);
    let events = {
        let borrowed = world.resource::<Borrowed>();
        weapons.step(
            &mut combatants,
            borrowed.world(),
            seed,
            tick,
            TICK_DURATION.as_millis() as u32,
        )
    };

    for &(slot, entity) in &players {
        let combatant = std::mem::take(&mut combatants[usize::from(slot)]);
        let mut entity = world.entity_mut(entity);
        entity.insert((combatant.inventory, WeaponCmd(combatant.input)));
        if let Some(mut walker) = entity.get_mut::<Walker>() {
            for axis in 0..3 {
                walker.0.state.velocity[axis] += combatant.impulse[axis];
            }
        }
    }
    sync_shots(world, weapons);
    world.resource_mut::<Outbox>().weapons.extend(events);
}

/// Makes the [`Shot`] entities match the stepped projectile set.
fn sync_shots(world: &mut World, weapons: Weapons) {
    let (projectiles, next_id) = weapons.into_parts();
    world.resource_mut::<ShotIds>().0 = next_id;
    let current: Vec<(Entity, u32)> = world
        .query::<(Entity, &Shot)>()
        .iter(world)
        .map(|(entity, shot)| (entity, shot.0.id))
        .collect();
    for (entity, id) in &current {
        if !projectiles.iter().any(|p| p.id == *id) {
            world.despawn(*entity);
        }
    }
    for projectile in projectiles {
        match current.iter().find(|(_, id)| *id == projectile.id) {
            Some(&(entity, _)) => {
                world.entity_mut(entity).insert(Shot(projectile));
            }
            None => {
                world.spawn(Shot(projectile));
            }
        }
    }
}

/// Weapon events queued since the last call.
pub(super) fn take_events(world: &mut World) -> Vec<WeaponEvent> {
    std::mem::take(&mut world.resource_mut::<Outbox>().weapons)
}
