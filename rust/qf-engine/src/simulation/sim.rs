//! The engine-owned simulation interface.
//!
//! [`Sim`] is the one object a server, a client or a test drives: push usercmds, run
//! fixed 8 ms ticks, drain [`SimEvent`]s and read a [`SimView`]. It wraps the private
//! entity world, so callers never see a Bevy type or entity id (players are addressed
//! by slot, map entities by lump index). It owns no wall clock: the caller decides how
//! many ticks to run, as [`ModeSimulation`](super::walk::ModeSimulation) does with its
//! accumulator. It is GPU- and window-free.

use super::entities::{BrushModels, MapEntities, MoverPose, TriggerEvent};
use super::player::{StepError, TICK_DURATION};
use super::pmove::{PmoveWorld, PredictedEvent, Vec3};
use super::walk::{WalkCommand, WalkPlayer};
use crate::game::{PlayerInput, Projectile, WeaponEvent, WeaponId};

/// Everything the simulation reports, in the order it happened within a tick.
#[derive(Clone, Debug, PartialEq)]
pub enum SimEvent {
    /// A movement event predicted by `pmove` for the player of `slot`.
    Pmove { slot: u8, event: PredictedEvent },
    /// A map entity fired.
    Trigger(TriggerEvent),
    /// A shot, projectile, impact or damage from the weapon system.
    Weapon(WeaponEvent),
}

impl From<WeaponEvent> for SimEvent {
    fn from(event: WeaponEvent) -> Self {
        Self::Weapon(event)
    }
}

impl From<TriggerEvent> for SimEvent {
    fn from(event: TriggerEvent) -> Self {
        Self::Trigger(event)
    }
}

impl From<(u8, PredictedEvent)> for SimEvent {
    fn from((slot, event): (u8, PredictedEvent)) -> Self {
        Self::Pmove { slot, event }
    }
}

/// Read-only pose of a player, for rendering and audio.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerView {
    pub slot: u8,
    pub eye: Vec3,
    pub view_angles: Vec3,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub ground_entity: Option<i32>,
    /// Selected weapon and its ammo, for the HUD.
    pub weapon: Option<WeaponId>,
    pub ammo: u32,
}

/// Read-only snapshot of what a renderer needs.
#[derive(Clone, Debug, PartialEq)]
pub struct SimView {
    pub tick: u64,
    pub players: Vec<PlayerView>,
    pub movers: Vec<MoverPose>,
    pub projectiles: Vec<Projectile>,
}

/// Fixed-tick simulation over a map and its players.
#[derive(Debug)]
pub struct Sim {
    map: MapEntities,
    events: Vec<SimEvent>,
}

impl Sim {
    /// Starts a simulation of `map`; `seed` feeds the deterministic random streams.
    pub fn new(mut map: MapEntities, seed: u64) -> Self {
        map.set_seed(seed);
        Self {
            map,
            events: Vec::new(),
        }
    }

    pub fn add_player(&mut self, slot: u8, player: WalkPlayer) {
        self.map.set_player(slot, player);
    }

    pub fn remove_player(&mut self, slot: u8) -> bool {
        self.map.remove_player(slot)
    }

    /// The usercmd the player of `slot` applies on every following tick until replaced.
    pub fn push_input(&mut self, slot: u8, command: WalkCommand) {
        self.map.push_input(slot, command);
    }

    /// Runs one 8 ms tick; the level clock advances by exactly one tick.
    pub fn tick(
        &mut self,
        collision: &dyn PmoveWorld,
        models: &dyn BrushModels,
    ) -> Result<(), StepError> {
        let time_ms = (self.map.tick() as i64 + 1) * TICK_DURATION.as_millis() as i64;
        let result = self.map.tick_players(collision, models, time_ms);
        self.events.extend(
            self.map
                .take_player_events()
                .into_iter()
                .map(SimEvent::from),
        );
        self.events.extend(
            self.map
                .take_weapon_events()
                .into_iter()
                .map(SimEvent::from),
        );
        self.events
            .extend(self.map.drain_events().into_iter().map(SimEvent::from));
        result
    }

    /// The weapon buttons the player of `slot` applies on every following tick (the
    /// attack button stays held until replaced; select and cycle apply once).
    pub fn push_weapon_input(&mut self, slot: u8, input: PlayerInput) {
        self.map.push_weapon_input(slot, input);
    }

    pub fn inventory(&mut self, slot: u8) -> Option<&crate::game::Inventory> {
        self.map.inventory(slot)
    }

    pub fn inventory_mut(&mut self, slot: u8) -> Option<&mut crate::game::Inventory> {
        self.map.inventory_mut(slot)
    }

    /// Dead players neither fire nor can be hit.
    pub fn set_alive(&mut self, slot: u8, alive: bool) {
        self.map.set_alive(slot, alive);
    }

    /// Takes the events produced since the last call.
    pub fn drain_events(&mut self) -> Vec<SimEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn view(&mut self) -> SimView {
        let players = self
            .map
            .player_slots()
            .into_iter()
            .filter_map(|slot| {
                let player = self.map.player(slot)?;
                let (eye, view_angles, origin, velocity, ground_entity) = (
                    player.eye_position(),
                    player.view_angles,
                    player.state.origin,
                    player.state.velocity,
                    player.ground_entity,
                );
                let inventory = self.map.inventory(slot);
                Some(PlayerView {
                    slot,
                    eye,
                    view_angles,
                    origin,
                    velocity,
                    ground_entity,
                    weapon: inventory.and_then(|i| i.selected()),
                    ammo: inventory
                        .and_then(|i| i.selected().map(|w| i.ammo(w)))
                        .unwrap_or(0),
                })
            })
            .collect();
        SimView {
            tick: self.map.tick(),
            players,
            movers: self.map.mover_poses(),
            projectiles: self.map.projectiles(),
        }
    }

    /// Hash of all simulated state; equal hashes mean equal simulations.
    pub fn state_hash(&self) -> u64 {
        self.map.state_hash()
    }

    /// The wrapped entity world, while callers still use its direct API.
    pub fn map(&self) -> &MapEntities {
        &self.map
    }

    pub fn map_mut(&mut self) -> &mut MapEntities {
        &mut self.map
    }
}
