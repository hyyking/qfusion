//! Offline gametype hosting: a map-agnostic engine side plus Rhai-owned rules.

pub mod combat;
pub mod host;
pub mod inventory;
pub mod items;
pub mod pickups;
pub mod roster;
pub mod vec;
pub mod weapons;

pub use combat::{Player, Team};
pub use host::{GameError, GameEvent, GameHost, GameWorld, OwnedBspWorld, TICK_MS, Value};
pub use inventory::{Inventory, WeaponState};
pub use items::{ItemDef, ItemId, ItemKind};
pub use pickups::{Pickup, PickupEvent, Pickups};
pub use weapons::{WeaponDef, WeaponId};

/// Built-in Bomb and Defuse gametype (port of the legacy `bomb` AngelScript gametype).
pub const BOMB_SCRIPT: &str = include_str!("../../scripts/gametypes/bomb.rhai");
