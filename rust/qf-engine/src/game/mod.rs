//! Offline gametype hosting: a map-agnostic engine side plus Rhai-owned rules.

pub mod combat;
pub mod host;
pub mod vec;

pub use combat::{Player, Team};
pub use host::{GameError, GameEvent, GameHost, GameWorld, OwnedBspWorld, TICK_MS, Value};
