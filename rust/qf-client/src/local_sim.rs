//! The client's local player on the engine [`Sim`].
//!
//! Replaces the client's use of `ModeSimulation`: this keeps the wall-clock accumulator
//! (the `Sim` owns none) and turns each fixed 8 ms tick's output into what the client
//! consumes: trigger events, weapon events, movement events and the per-tick log the
//! `.qfrec` recorder needs. The local player is slot 0, as before.

use std::time::Duration;

use qf_engine::game::{PlayerInput, WeaponEvent, WeaponId};
use qf_engine::simulation::entities::{BrushModels, MapEntities, TriggerEvent};
use qf_engine::simulation::player::{NoclipPlayer, StepError, TICK_DURATION};
use qf_engine::simulation::pmove::{PmoveWorld, PredictedEvent};
use qf_engine::simulation::sim::{Sim, SimEvent};
use qf_engine::simulation::walk::{TickLogEntry, WalkCommand, WalkPlayer};

use crate::weapon_input::WeaponInput;

pub const LOCAL_SLOT: u8 = 0;
const NOCLIP_EYE_HEIGHT: f32 = 30.0;
const SEED: u64 = 0;

/// What a frame's ticks produced, in tick order.
#[derive(Debug, Default)]
pub struct FrameEvents {
    pub ticks: usize,
    pub triggers: Vec<TriggerEvent>,
    pub weapons: Vec<WeaponEvent>,
    pub movement: Vec<PredictedEvent>,
}

pub struct LocalSim {
    sim: Sim,
    remainder: Duration,
    /// The local player after the last tick; `Sim` only hands out copies.
    walk: WalkPlayer,
    tick_log: Option<Vec<TickLogEntry>>,
}

impl LocalSim {
    /// Starts walking at the pose of `player` with every weapon at full ammo.
    pub fn new(player: &NoclipPlayer, entities: MapEntities) -> Self {
        let walk = WalkPlayer::from_noclip(player);
        let mut sim = Sim::new(entities, SEED);
        sim.add_player(LOCAL_SLOT, walk.clone());
        if let Some(inventory) = sim.inventory_mut(LOCAL_SLOT) {
            for weapon in WeaponId::ALL {
                inventory.give_weapon(weapon, weapon.def().ammo_max);
            }
        }
        Self {
            sim,
            remainder: Duration::ZERO,
            walk,
            tick_log: None,
        }
    }

    pub fn is_walking(&mut self) -> bool {
        !self.sim.noclip(LOCAL_SLOT)
    }

    /// The walking player, `None` while flying.
    pub fn walk(&mut self) -> Option<&WalkPlayer> {
        if self.is_walking() {
            Some(&self.walk)
        } else {
            None
        }
    }

    /// Lets `edit` change the walking player (spawn, movement lock); written back only
    /// when it returns true. Does nothing while flying.
    pub fn edit_walk(&mut self, edit: impl FnOnce(&mut WalkPlayer) -> bool) {
        if !self.is_walking() {
            return;
        }
        let mut walk = self.walk.clone();
        if edit(&mut walk) {
            self.sim.add_player(LOCAL_SLOT, walk.clone());
            self.walk = walk;
        }
    }

    pub fn set_noclip(&mut self, noclip: bool) {
        if self.sim.noclip(LOCAL_SLOT) != noclip {
            self.sim.set_noclip(LOCAL_SLOT, noclip);
        }
    }

    pub fn set_alive(&mut self, alive: bool) {
        self.sim.set_alive(LOCAL_SLOT, alive);
    }

    /// Eye position and view angles for rendering.
    pub fn view(&mut self) -> ([f32; 3], [f32; 3]) {
        if self.is_walking() {
            (self.walk.eye_position(), self.walk.view_angles)
        } else {
            let mut eye = self.walk.state.origin;
            eye[2] += NOCLIP_EYE_HEIGHT;
            (eye, self.walk.view_angles)
        }
    }

    /// Velocity in units per second; flying has none.
    pub fn velocity(&mut self) -> [f32; 3] {
        if self.is_walking() {
            self.walk.state.velocity
        } else {
            [0.0; 3]
        }
    }

    pub fn inventory(&mut self) -> Option<&qf_engine::game::Inventory> {
        self.sim.inventory(LOCAL_SLOT)
    }

    pub fn movers(&self) -> Vec<qf_engine::simulation::entities::MoverPose> {
        self.sim.map().mover_poses()
    }

    /// Starts or stops logging walking ticks for the recorder. Logging only copies tick
    /// results, so it never changes the simulation.
    pub fn set_tick_logging(&mut self, enabled: bool) {
        self.tick_log = enabled.then(Vec::new);
    }

    pub fn take_tick_log(&mut self) -> Vec<TickLogEntry> {
        self.tick_log
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Runs the whole 8 ms ticks that `elapsed` adds, applying `command` and `weapons`
    /// to every one of them.
    pub fn advance(
        &mut self,
        world: &dyn PmoveWorld,
        models: &dyn BrushModels,
        elapsed: Duration,
        command: WalkCommand,
        weapons: &mut WeaponInput,
    ) -> Result<FrameEvents, StepError> {
        let accumulated = self
            .remainder
            .checked_add(elapsed)
            .ok_or(StepError::ElapsedTimeOverflow)?;
        let ticks = accumulated.as_nanos() / TICK_DURATION.as_nanos();
        let ticks = usize::try_from(ticks).map_err(|_| StepError::TooManyTicks)?;
        self.sim.push_input(LOCAL_SLOT, command);
        let mut frame = FrameEvents {
            ticks,
            ..FrameEvents::default()
        };
        // One-shot select/cycle stay pending until a frame actually runs a tick.
        let weapons = if ticks > 0 {
            weapons.take()
        } else {
            PlayerInput::default()
        };
        for tick in 0..ticks {
            // Select and cycle apply once; only the first tick of the frame sees them.
            let input = if tick == 0 {
                weapons
            } else {
                PlayerInput {
                    select: None,
                    cycle: 0,
                    ..weapons
                }
            };
            self.sim.push_weapon_input(LOCAL_SLOT, input);
            self.sim.tick(world, models)?;
            let mut fired = 0;
            let mut teleported = false;
            for event in self.sim.drain_events() {
                match event {
                    SimEvent::Pmove { slot, event } if slot == LOCAL_SLOT => {
                        frame.movement.push(event)
                    }
                    SimEvent::Pmove { .. } => {}
                    SimEvent::Trigger(trigger) => {
                        fired += 1;
                        teleported |= matches!(trigger, TriggerEvent::Teleport { .. });
                        frame.triggers.push(trigger);
                    }
                    SimEvent::Weapon(event) => frame.weapons.push(event),
                }
            }
            if let Some(walk) = self.sim.map_mut().player(LOCAL_SLOT) {
                self.walk = walk.clone();
            }
            if teleported {
                let angles = self.walk.state.view_angles;
                self.walk.view_angles = [-angles[0], angles[1], angles[2]];
                self.sim.add_player(LOCAL_SLOT, self.walk.clone());
            }
            if self.tick_log.is_some() && self.is_walking() {
                let outcome = self.sim.map_mut().last_outcome(LOCAL_SLOT).cloned();
                if let (Some(log), Some(outcome)) = (&mut self.tick_log, outcome) {
                    log.push(TickLogEntry {
                        outcome,
                        state: self.walk.state,
                        trigger_events: fired,
                    });
                }
            }
        }
        self.remainder =
            Duration::from_nanos((accumulated.as_nanos() % TICK_DURATION.as_nanos()) as u64);
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qf_engine::simulation::pmove::PmTrace;

    struct Void;

    impl PmoveWorld for Void {
        fn trace(&self, _: [f32; 3], _: [f32; 3], _: [f32; 3], end: [f32; 3], _: u32) -> PmTrace {
            PmTrace::miss(end)
        }

        fn point_contents(&self, _: [f32; 3]) -> u32 {
            0
        }
    }

    struct NoModels;

    impl BrushModels for NoModels {
        fn model_bounds(&self, _: usize) -> Option<([f32; 3], [f32; 3])> {
            None
        }

        fn box_touches_model(&self, _: usize, _: [f32; 3], _: [f32; 3], _: [f32; 3]) -> bool {
            false
        }
    }

    fn local() -> LocalSim {
        let player = NoclipPlayer {
            position: [0.0; 3],
            view_angles: [0.0; 3],
        };
        LocalSim::new(&player, MapEntities::spawn(&[], &NoModels))
    }

    #[test]
    fn starts_with_a_loadout_for_the_hud() {
        let mut sim = local();
        let inventory = sim.inventory().expect("local inventory");
        assert_eq!(inventory.owned().count(), WeaponId::ALL.len());
        assert!(inventory.selected().is_some());
    }

    #[test]
    fn firing_spends_ammo_and_reports_weapon_events() {
        let mut sim = local();
        let mut input = WeaponInput::default();
        input.select_key("2");
        let before = sim.inventory().unwrap().ammo(WeaponId::Machinegun);
        let mut fired = false;
        for _ in 0..30 {
            input.set_fire(true);
            let frame = sim
                .advance(
                    &Void,
                    &NoModels,
                    TICK_DURATION * 4,
                    WalkCommand::default(),
                    &mut input,
                )
                .unwrap();
            fired |= frame
                .weapons
                .iter()
                .any(|event| matches!(event, WeaponEvent::Fired { .. }));
        }
        assert!(fired);
        assert!(sim.inventory().unwrap().ammo(WeaponId::Machinegun) < before);
    }

    #[test]
    fn same_inputs_give_the_same_state() {
        let run = || {
            let mut sim = local();
            let mut input = WeaponInput::default();
            input.select_key("2");
            input.set_fire(true);
            for _ in 0..20 {
                let command = WalkCommand {
                    forward: 1.0,
                    ..WalkCommand::default()
                };
                sim.advance(&Void, &NoModels, TICK_DURATION * 3, command, &mut input)
                    .unwrap();
            }
            (sim.sim.state_hash(), sim.view())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn select_survives_frames_without_ticks() {
        let mut sim = local();
        let mut input = WeaponInput::default();
        input.select_key("3");
        let frame = sim
            .advance(
                &Void,
                &NoModels,
                Duration::from_millis(1),
                WalkCommand::default(),
                &mut input,
            )
            .unwrap();
        assert_eq!(frame.ticks, 0);
        assert_eq!(input.take().select, Some(WeaponId::Riotgun));
    }
}
