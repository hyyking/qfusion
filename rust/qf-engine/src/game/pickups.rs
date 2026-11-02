//! Item pickups placed in the world: touch, apply to the player, respawn.
//!
//! Pure state advanced by the caller with a level time in milliseconds, so it
//! stays deterministic at the fixed tick and can be turned into ECS components
//! (`Pickup`, `PickupState`) without changing the rules, which live in
//! [`apply_item`].
//!
//! Known differences from the legacy game: items are not dropped to the floor,
//! mega health respawns on a fixed timer instead of waiting for the owner's health
//! to decay, and dropped items, ammo packs and `_weak` ammo are not supported.

use std::collections::BTreeMap;

use crate::content::bsp::entities::Entity;
use crate::simulation::pmove::Vec3;

use super::combat::{Player, START_HEALTH};
use super::items::{ARMOR_GA_MAX, ARMOR_YA_MAX, ItemDef, ItemId, ItemKind, item_by_classname};

const ITEM_MINS: Vec3 = [-15.0, -15.0, -15.0];
const ITEM_MAXS: Vec3 = [15.0, 15.0, 31.0];
const PLAYER_MINS: Vec3 = [-16.0, -16.0, -24.0];
const PLAYER_MAXS: Vec3 = [16.0, 16.0, 40.0];
const MAX_HEALTH_IGNORING_LIMIT: f32 = 200.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickupState {
    Available,
    RespawnAt(u64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pickup {
    pub item: ItemId,
    pub origin: Vec3,
    /// Position of the entity in the map's entity lump.
    pub entity: usize,
    pub state: PickupState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickupEvent {
    PickedUp {
        pickup: usize,
        item: ItemId,
        player: usize,
    },
    Respawned {
        pickup: usize,
        item: ItemId,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Pickups {
    items: Vec<Pickup>,
}

impl Pickups {
    /// Instantiates the item entities of a map entity lump. Returns the number of
    /// item-like classnames that are not supported, by classname.
    pub fn from_entities(lump: &[Entity]) -> (Self, BTreeMap<String, usize>) {
        let mut items = Vec::new();
        let mut unsupported = BTreeMap::new();
        for (entity, ent) in lump.iter().enumerate() {
            let Some(classname) = ent.get("classname") else {
                continue;
            };
            let lower = classname.to_ascii_lowercase();
            match item_by_classname(&lower) {
                Some(item) => items.push(Pickup {
                    item,
                    origin: ent.get("origin").and_then(parse_vec3).unwrap_or([0.0; 3]),
                    entity,
                    state: PickupState::Available,
                }),
                None if is_item_classname(&lower) => *unsupported.entry(lower).or_default() += 1,
                None => {}
            }
        }
        (Self { items }, unsupported)
    }

    pub fn all(&self) -> &[Pickup] {
        &self.items
    }

    pub fn get(&self, index: usize) -> Option<&Pickup> {
        self.items.get(index)
    }

    /// Makes pickups whose respawn time has come available again.
    pub fn update(&mut self, now_ms: u64) -> Vec<PickupEvent> {
        let mut events = Vec::new();
        for (index, pickup) in self.items.iter_mut().enumerate() {
            if let PickupState::RespawnAt(at) = pickup.state
                && now_ms >= at
            {
                pickup.state = PickupState::Available;
                events.push(PickupEvent::Respawned {
                    pickup: index,
                    item: pickup.item,
                });
            }
        }
        events
    }

    /// Touch pass for one player: every available item overlapping the player is
    /// applied and, when taken, starts its respawn timer. Items are visited in
    /// lump order.
    pub fn touch(
        &mut self,
        now_ms: u64,
        player_id: usize,
        player: &mut Player,
    ) -> Vec<PickupEvent> {
        let mut events = Vec::new();
        if !player.alive {
            return events;
        }
        let (mins, maxs) = (
            add(player.origin, PLAYER_MINS),
            add(player.origin, PLAYER_MAXS),
        );
        for (index, pickup) in self.items.iter_mut().enumerate() {
            if pickup.state != PickupState::Available {
                continue;
            }
            let (item_mins, item_maxs) =
                (add(pickup.origin, ITEM_MINS), add(pickup.origin, ITEM_MAXS));
            if !overlaps(mins, maxs, item_mins, item_maxs) {
                continue;
            }
            let def = pickup.item.def();
            if apply_item(player, def) {
                pickup.state = PickupState::RespawnAt(now_ms + u64::from(def.respawn_ms));
                events.push(PickupEvent::PickedUp {
                    pickup: index,
                    item: pickup.item,
                    player: player_id,
                });
            }
        }
        events
    }
}

fn is_item_classname(classname: &str) -> bool {
    ["weapon_", "ammo_", "item_"]
        .iter()
        .any(|prefix| classname.starts_with(prefix))
}

fn parse_vec3(text: &str) -> Option<Vec3> {
    let parts: Vec<f32> = text
        .split_ascii_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let vector: Vec3 = parts.try_into().ok()?;
    vector.iter().all(|c| c.is_finite()).then_some(vector)
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|axis| a[axis] + b[axis])
}

fn overlaps(a_mins: Vec3, a_maxs: Vec3, b_mins: Vec3, b_maxs: Vec3) -> bool {
    (0..3).all(|axis| a_mins[axis] <= b_maxs[axis] && a_maxs[axis] >= b_mins[axis])
}

/// Armor tier ceiling for the current armor (`GS_Armor_TagForCount`).
fn armor_tier_max(armor: f32) -> f32 {
    let count = armor.floor();
    if count > ARMOR_YA_MAX as f32 {
        150.0
    } else if count > ARMOR_GA_MAX as f32 {
        ARMOR_YA_MAX as f32
    } else if count > 0.0 {
        ARMOR_GA_MAX as f32
    } else {
        0.0
    }
}

/// Applies an item to the player; returns false when the player gains nothing,
/// in which case the item stays in the world (`G_PickupItem`).
pub fn apply_item(player: &mut Player, def: &ItemDef) -> bool {
    match def.kind {
        ItemKind::Weapon(weapon) => player.inventory.pickup_weapon(weapon),
        ItemKind::Ammo(weapon) => player.inventory.add_ammo(weapon, weapon.def().ammo_pickup),
        ItemKind::Armor => {
            let pickup_max = def.max as f32;
            if player.armor >= pickup_max {
                return false;
            }
            let tier_max = armor_tier_max(player.armor);
            let ceiling = if tier_max == 0.0 {
                pickup_max
            } else {
                tier_max.max(pickup_max)
            };
            let new = (player.armor + def.quantity as f32).min(ceiling);
            if new <= player.armor {
                return false;
            }
            player.armor = new;
            true
        }
        ItemKind::Health { ignore_max } => {
            if !ignore_max && player.health >= START_HEALTH {
                return false;
            }
            let limit = if ignore_max {
                MAX_HEALTH_IGNORING_LIMIT
            } else {
                START_HEALTH
            };
            player.health = (player.health + def.quantity as f32).min(limit);
            true
        }
        ItemKind::Powerup(powerup) => {
            player.inventory.add_powerup(powerup, def.quantity + 1);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::combat::Team;
    use crate::game::weapons::WeaponId;

    fn ent(pairs: &[(&str, &str)]) -> Entity {
        Entity {
            properties: pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        }
    }

    fn player_at(origin: Vec3) -> Player {
        let mut player = Player::new(Team::Alpha);
        player.alive = true;
        player.health = 100.0;
        player.origin = origin;
        player
    }

    fn lump() -> Vec<Entity> {
        vec![
            ent(&[("classname", "worldspawn")]),
            ent(&[("classname", "weapon_rocketlauncher"), ("origin", "0 0 0")]),
            ent(&[("classname", "item_armor_ya"), ("origin", "500 0 0")]),
            ent(&[
                ("classname", "ammo_rocketlauncher_weak"),
                ("origin", "0 0 0"),
            ]),
            ent(&[("classname", "item_ammopack")]),
        ]
    }

    #[test]
    fn spawns_items_and_reports_unsupported() {
        let (pickups, unsupported) = Pickups::from_entities(&lump());
        assert_eq!(pickups.all().len(), 2);
        assert_eq!(pickups.all()[0].entity, 1);
        assert_eq!(unsupported.len(), 2);
        assert_eq!(unsupported["item_ammopack"], 1);
    }

    #[test]
    fn touch_collects_and_respawns() {
        let (mut pickups, _) = Pickups::from_entities(&lump());
        let mut player = player_at([10.0, 0.0, 24.0]);
        let events = pickups.touch(1_000, 0, &mut player);
        assert_eq!(events.len(), 1);
        assert!(player.inventory.owns(WeaponId::RocketLauncher));
        assert_eq!(player.inventory.ammo(WeaponId::RocketLauncher), 10);
        assert_eq!(pickups.all()[0].state, PickupState::RespawnAt(6_000));

        assert!(pickups.touch(1_008, 0, &mut player).is_empty(), "taken");
        assert!(pickups.update(5_999).is_empty());
        let events = pickups.update(6_000);
        assert!(matches!(
            events[..],
            [PickupEvent::Respawned { pickup: 0, .. }]
        ));
        assert_eq!(pickups.all()[0].state, PickupState::Available);
    }

    #[test]
    fn far_or_dead_players_collect_nothing() {
        let (mut pickups, _) = Pickups::from_entities(&lump());
        let mut far = player_at([100.0, 0.0, 24.0]);
        assert!(pickups.touch(0, 0, &mut far).is_empty());
        let mut dead = player_at([0.0, 0.0, 24.0]);
        dead.alive = false;
        assert!(pickups.touch(0, 0, &mut dead).is_empty());
    }

    #[test]
    fn full_players_leave_items_in_the_world() {
        let (mut pickups, _) = Pickups::from_entities(&lump());
        let mut player = player_at([0.0, 0.0, 24.0]);
        player.inventory.give_weapon(WeaponId::RocketLauncher, 10);
        assert!(pickups.touch(0, 0, &mut player).is_empty());
        assert_eq!(pickups.all()[0].state, PickupState::Available);
    }

    #[test]
    fn armor_follows_legacy_tiers() {
        let ya = item_by_classname("item_armor_ya").unwrap().def();
        let ga = item_by_classname("item_armor_ga").unwrap().def();
        let shard = item_by_classname("item_armor_shard").unwrap().def();
        let mut player = player_at([0.0; 3]);
        assert!(apply_item(&mut player, ya));
        assert_eq!(player.armor, 75.0);
        assert!(apply_item(&mut player, ya));
        assert_eq!(player.armor, 125.0, "capped at the yellow ceiling");
        assert!(!apply_item(&mut player, ya));
        assert!(!apply_item(&mut player, ga), "green is capped at 100");
        assert!(apply_item(&mut player, shard));
        assert_eq!(player.armor, 130.0);
    }

    #[test]
    fn health_respects_limits() {
        let medium = item_by_classname("item_health_medium").unwrap().def();
        let small = item_by_classname("item_health_small").unwrap().def();
        let mega = item_by_classname("item_health_mega").unwrap().def();
        let mut player = player_at([0.0; 3]);
        player.health = 90.0;
        assert!(apply_item(&mut player, medium));
        assert_eq!(player.health, 100.0);
        assert!(!apply_item(&mut player, medium));
        assert!(apply_item(&mut player, small));
        assert_eq!(player.health, 105.0);
        assert!(apply_item(&mut player, mega));
        assert_eq!(player.health, 200.0);
    }

    #[test]
    fn powerups_add_seconds() {
        let quad = item_by_classname("item_quad").unwrap().def();
        let mut player = player_at([0.0; 3]);
        assert!(apply_item(&mut player, quad));
        assert_eq!(
            player
                .inventory
                .powerup_seconds(crate::game::items::Powerup::Quad),
            31
        );
    }
}
