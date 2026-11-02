//! Gametype player roster stored in a `bevy_ecs` world.
//!
//! One entity per player slot holds the engine-owned [`Player`] state; the movement
//! lock is a resource. Scripts and the host reach it only through [`Roster`], which
//! keeps slots stable and iterates in slot order, so no Bevy type or query order is
//! observable. This is the storage that later joins the map entity world.

use std::collections::BTreeMap;

use bevy_ecs::prelude::*;

use super::combat::{Player, Team};
use super::host::Value;

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Slot(usize);

#[derive(Component, Debug)]
struct GamePlayer(Player);

/// `set_movement_locked`: whether player movement input is frozen by the gametype.
#[derive(Resource, Debug)]
struct MovementLock(bool);

/// Per-tick published HUD values, by slot.
#[derive(Resource, Debug, Default)]
struct HudState(BTreeMap<usize, BTreeMap<String, Value>>);

/// Slot -> entity, dense and stable.
#[derive(Resource, Debug, Default)]
struct SlotIndex(Vec<Entity>);

/// The gametype's view of the players stored in the shared entity world. Create one per
/// access with [`Roster::new`] (cheap); it borrows the world.
#[derive(Debug)]
pub struct Roster<'w> {
    world: &'w mut World,
}

impl<'w> Roster<'w> {
    /// Registers the roster resources in `world` once. Movement starts locked until the
    /// script opens the round.
    pub fn init(world: &mut World) {
        world.insert_resource(MovementLock(true));
        world.insert_resource(HudState::default());
        world.insert_resource(SlotIndex::default());
    }

    pub fn new(world: &'w mut World) -> Self {
        Self { world }
    }

    pub fn len(&self) -> usize {
        self.world.resource::<SlotIndex>().0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds a player and returns its slot.
    pub fn add(&mut self, team: Team) -> usize {
        let slot = self.len();
        let entity = self
            .world
            .spawn((Slot(slot), GamePlayer(Player::new(team))))
            .id();
        self.world.resource_mut::<SlotIndex>().0.push(entity);
        slot
    }

    fn entity(&self, slot: usize) -> Option<Entity> {
        self.world.resource::<SlotIndex>().0.get(slot).copied()
    }

    pub fn get(self, slot: usize) -> Option<&'w Player> {
        let entity = self.entity(slot)?;
        let world: &'w World = self.world;
        world.get::<GamePlayer>(entity).map(|p| &p.0)
    }

    pub fn get_mut(self, slot: usize) -> Option<&'w mut Player> {
        let entity = self.entity(slot)?;
        self.world
            .get_mut::<GamePlayer>(entity)
            .map(|p| &mut p.into_inner().0)
    }

    pub fn movement_locked(&self) -> bool {
        self.world.resource::<MovementLock>().0
    }

    pub fn publish_hud(&mut self, slot: usize, values: BTreeMap<String, Value>) {
        self.world.resource_mut::<HudState>().0.insert(slot, values);
    }

    pub fn hud(self, slot: usize) -> Option<&'w BTreeMap<String, Value>> {
        let world: &'w World = self.world;
        world.resource::<HudState>().0.get(&slot)
    }

    pub fn set_movement_locked(&mut self, locked: bool) {
        self.world.resource_mut::<MovementLock>().0 = locked;
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut world = World::new();
        Roster::init(&mut world);
        world
    }

    #[test]
    fn slots_are_dense_and_stable() {
        let mut world = world();
        assert!(Roster::new(&mut world).is_empty());
        assert_eq!(Roster::new(&mut world).add(Team::Alpha), 0);
        assert_eq!(Roster::new(&mut world).add(Team::Beta), 1);
        Roster::new(&mut world).get_mut(0).unwrap().health = 42.0;
        assert_eq!(Roster::new(&mut world).len(), 2);
        assert_eq!(Roster::new(&mut world).get(0).unwrap().health, 42.0);
        assert_eq!(Roster::new(&mut world).get(1).unwrap().team, Team::Beta);
        assert!(Roster::new(&mut world).get(2).is_none());
    }

    #[test]
    fn movement_starts_locked() {
        let mut world = world();
        assert!(Roster::new(&mut world).movement_locked());
        Roster::new(&mut world).set_movement_locked(false);
        assert!(!Roster::new(&mut world).movement_locked());
    }
}
