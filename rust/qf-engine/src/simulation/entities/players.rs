//! Players as entities of the map world: one entity per slot with its usercmd and
//! movement state, moved by the `Movement` system of the schedule.
//!
//! `pmove` stays a pure function: the system takes the [`WalkPlayer`] out of its
//! component, builds the movement command from the [`WalkCommand`] and calls it with
//! a touch hook bound to this world. The hook runs at the same point of the move as
//! the legacy `PMoveTouchTriggers`, so touches (which may change the move's state)
//! behave exactly as through [`PlayerTouch`](super::PlayerTouch).

use bevy_ecs::prelude::*;

use crate::game::Inventory;
use crate::simulation::player::StepError;
use crate::simulation::pmove::{PmoveState, PmoveTouch, PredictedEvent, TouchContext};
use crate::simulation::walk::{TickOutcome, WalkCommand, WalkPlayer};

use super::components::{Player, PlayerPresence};
use super::movers::{self, MoverWorld};
use super::schedule::{Borrowed, take_predicted};
use super::{BrushModels, logic};

/// Player slot; systems visit players in ascending slot order.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Slot(pub u8);

/// The usercmd applied by the next tick.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Cmd(pub WalkCommand);

/// Movement state and rules of the player.
#[derive(Component, Clone, Debug)]
pub(super) struct Walker(pub WalkPlayer);

/// What the last tick fed to and got back from `pmove`.
#[derive(Component, Clone, Debug, Default)]
pub(super) struct LastTick(pub Option<TickOutcome>);

/// First error of a run, reported to the caller after it.
#[derive(Resource, Default)]
pub(super) struct Fault(pub Option<StepError>);

pub(super) fn spawn(world: &mut World, slot: u8, player: WalkPlayer) {
    match entity_of(world, slot) {
        Some(entity) => {
            world.entity_mut(entity).insert(Walker(player));
        }
        None => {
            world.spawn((
                Slot(slot),
                Cmd::default(),
                Walker(player),
                LastTick::default(),
                Inventory::default(),
                super::weapons::WeaponCmd::default(),
                super::weapons::Alive(true),
            ));
        }
    }
}

pub(super) fn despawn(world: &mut World, slot: u8) -> bool {
    entity_of(world, slot).is_some_and(|entity| world.despawn(entity))
}

pub(super) fn entity_of(world: &mut World, slot: u8) -> Option<Entity> {
    world
        .query::<(Entity, &Slot)>()
        .iter(world)
        .find(|(_, s)| s.0 == slot)
        .map(|(entity, _)| entity)
}

/// Players in ascending slot order.
pub(super) fn in_slot_order(world: &mut World) -> Vec<(u8, Entity)> {
    let mut players: Vec<(u8, Entity)> = world
        .query::<(Entity, &Slot)>()
        .iter(world)
        .map(|(entity, slot)| (slot.0, entity))
        .collect();
    players.sort_unstable_by_key(|&(slot, _)| slot);
    players
}

/// Players in ascending slot order, without needing `&mut World`.
pub(super) fn in_slot_order_ro(world: &World) -> Vec<(u8, Entity)> {
    let Some(mut query) = world.try_query::<(Entity, &Slot)>() else {
        return Vec::new();
    };
    let mut players: Vec<(u8, Entity)> = query
        .iter(world)
        .map(|(entity, slot)| (slot.0, entity))
        .collect();
    players.sort_unstable_by_key(|&(slot, _)| slot);
    players
}

/// Touch hook run inside `pmove`: thinks and the touch pass for the moving player.
struct TouchHook<'a> {
    world: &'a mut World,
    models: &'a dyn BrushModels,
}

impl PmoveTouch for TouchHook<'_> {
    fn touch_triggers(
        &mut self,
        state: &mut PmoveState,
        context: &TouchContext,
        events: &mut Vec<PredictedEvent>,
    ) {
        let world = &mut *self.world;
        world.resource_mut::<Player>().0 = Some(PlayerPresence {
            state: *state,
            context: *context,
        });
        let swept = logic::swept_bounds(state, context.previous_origin);
        logic::run_thinks(world);
        logic::touch_pass(world, self.models, swept);
        if let Some(player) = world.resource_mut::<Player>().0.take() {
            *state = player.state;
        }
        take_predicted(world, events);
    }
}

/// The `Movement` system: every player moves once, in slot order.
pub(super) fn movement(world: &mut World) {
    let time_ms = world.resource::<super::components::Level>().time_ms;
    let players = in_slot_order(world);
    world.resource_scope(|world, borrowed: Mut<Borrowed>| {
        let (models, collision) = (borrowed.models(), borrowed.world());
        for (_, entity) in players {
            let Some(Walker(mut walker)) = world.entity_mut(entity).take::<Walker>() else {
                continue;
            };
            let command = world
                .get::<Cmd>(entity)
                .map_or_else(Default::default, |c| c.0);
            let solid = MoverWorld::new(collision, models, movers::poses(world));
            let result = {
                let mut hook = TouchHook { world, models };
                walker.step_with_outcome(&solid, command, Some(&mut hook))
            };
            match result {
                Ok(outcome) => {
                    for &touched in &walker.touched {
                        if let Ok(lump) = usize::try_from(touched) {
                            movers::touched_by_player(world, lump, time_ms);
                        }
                    }
                    world
                        .entity_mut(entity)
                        .insert((Walker(walker), LastTick(Some(outcome))));
                }
                Err(error) => {
                    world.entity_mut(entity).insert(Walker(walker));
                    let fault = &mut world.resource_mut::<Fault>().0;
                    fault.get_or_insert(error);
                }
            }
        }
    });
}
