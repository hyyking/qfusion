//! Gametype player roster stored in a `bevy_ecs` world.
//!
//! One entity per player slot holds the engine-owned [`Player`] state; the movement
//! lock is a resource. Scripts and the host reach it only through [`Roster`], which
//! keeps slots stable and iterates in slot order, so no Bevy type or query order is
//! observable. This is the storage that later joins the map entity world.

use bevy_ecs::prelude::*;

use super::combat::{Player, Team};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Slot(usize);

#[derive(Component, Debug)]
struct GamePlayer(Player);

/// `set_movement_locked`: whether player movement input is frozen by the gametype.
#[derive(Resource, Debug)]
struct MovementLock(bool);

#[derive(Debug)]
pub struct Roster {
    world: World,
    by_slot: Vec<Entity>,
}

impl Default for Roster {
    fn default() -> Self {
        Self::new()
    }
}

impl Roster {
    /// Movement starts locked until the script opens the round.
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(MovementLock(true));
        Self {
            world,
            by_slot: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.by_slot.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_slot.is_empty()
    }

    /// Adds a player and returns its slot.
    pub fn add(&mut self, team: Team) -> usize {
        let slot = self.by_slot.len();
        let entity = self
            .world
            .spawn((Slot(slot), GamePlayer(Player::new(team))))
            .id();
        self.by_slot.push(entity);
        slot
    }

    pub fn get(&self, slot: usize) -> Option<&Player> {
        let entity = *self.by_slot.get(slot)?;
        self.world.get::<GamePlayer>(entity).map(|p| &p.0)
    }

    pub fn get_mut(&mut self, slot: usize) -> Option<&mut Player> {
        let entity = *self.by_slot.get(slot)?;
        self.world
            .get_mut::<GamePlayer>(entity)
            .map(|p| &mut p.into_inner().0)
    }

    pub fn movement_locked(&self) -> bool {
        self.world.resource::<MovementLock>().0
    }

    pub fn set_movement_locked(&mut self, locked: bool) {
        self.world.resource_mut::<MovementLock>().0 = locked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_dense_and_stable() {
        let mut roster = Roster::new();
        assert!(roster.is_empty());
        assert_eq!(roster.add(Team::Alpha), 0);
        assert_eq!(roster.add(Team::Beta), 1);
        roster.get_mut(0).unwrap().health = 42.0;
        assert_eq!(roster.len(), 2);
        assert_eq!(roster.get(0).unwrap().health, 42.0);
        assert_eq!(roster.get(1).unwrap().team, Team::Beta);
        assert!(roster.get(2).is_none());
    }

    #[test]
    fn movement_starts_locked() {
        let mut roster = Roster::new();
        assert!(roster.movement_locked());
        roster.set_movement_locked(false);
        assert!(!roster.movement_locked());
    }
}
