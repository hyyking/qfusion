use std::time::Duration;

use super::entities::{BrushModels, MapEntities, TriggerEvent};
use super::player::{NoclipPlayer, PlayerCommand, StepError, TICK_DURATION};
use super::pmove::{
    BUTTON_SPECIAL, PmType, PmoveCommand, PmoveRules, PmoveState, PmoveTouch, PmoveWorld,
    angle_to_short, pmove, pmove_with_touch,
};

pub const MOVE_AXIS: f32 = 127.0;
const NOCLIP_EYE_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WalkCommand {
    /// Movement axes in [-1, 1]; `up` jumps when positive and crouches when negative.
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    /// Dash, wall jump and double jump button.
    pub special: bool,
    /// Pitch (positive looks up), yaw and roll in degrees.
    pub view_angles: [f32; 3],
}

/// Walking player driven by the shared `pmove` implementation at the fixed tick.
#[derive(Clone, Debug)]
pub struct WalkPlayer {
    pub state: PmoveState,
    pub view_angles: [f32; 3],
    pub rules: PmoveRules,
}

impl WalkPlayer {
    /// Starts walking at the pose of a noclip player, with its view angles.
    pub fn from_noclip(player: &NoclipPlayer) -> Self {
        let mut walk = Self {
            state: PmoveState::new(player.position),
            view_angles: player.view_angles,
            rules: PmoveRules::default(),
        };
        walk.state.stats.features = u32::MAX;
        walk
    }

    /// Noclip pose at this player's eye so the mode can be swapped in place.
    pub fn to_noclip(&self) -> NoclipPlayer {
        NoclipPlayer {
            position: self.state.origin,
            view_angles: self.view_angles,
        }
    }

    pub fn eye_position(&self) -> [f32; 3] {
        let mut eye = self.state.origin;
        eye[2] += self.state.view_height;
        eye
    }

    pub fn step<W: PmoveWorld>(
        &mut self,
        world: &W,
        command: WalkCommand,
    ) -> Result<(), StepError> {
        self.step_with_touch(world, command, None)
    }

    /// `step` that also lets map triggers act on the player during the move.
    pub fn step_with_touch<W: PmoveWorld>(
        &mut self,
        world: &W,
        command: WalkCommand,
        touch: Option<&mut dyn PmoveTouch>,
    ) -> Result<(), StepError> {
        let values = [command.forward, command.right, command.up]
            .into_iter()
            .chain(command.view_angles);
        if !values.into_iter().all(f32::is_finite) {
            return Err(StepError::NonFiniteInput);
        }
        let axis = |value: f32| (value.clamp(-1.0, 1.0) * MOVE_AXIS) as i8;

        self.view_angles = command.view_angles;
        // Legacy pitch is positive downward.
        let angles = [
            -command.view_angles[0],
            command.view_angles[1],
            command.view_angles[2],
        ]
        .map(|angle| angle_to_short(angle) as i16);
        let pm_command = PmoveCommand {
            msec: TICK_DURATION.as_millis() as u8,
            buttons: if command.special { BUTTON_SPECIAL } else { 0 },
            forward_move: axis(command.forward),
            side_move: axis(command.right),
            up_move: axis(command.up),
            angles,
        };
        self.state.pm_type = PmType::Normal;
        match touch {
            Some(touch) => {
                pmove_with_touch(world, &self.rules, &mut self.state, &pm_command, touch);
            }
            None => {
                pmove(world, &self.rules, &mut self.state, &pm_command);
            }
        }
        Ok(())
    }
}

/// Which movement implementation drives the local player.
#[derive(Clone, Debug)]
pub enum MovementMode {
    Noclip(NoclipPlayer),
    Walk(WalkPlayer),
}

impl MovementMode {
    pub fn is_walking(&self) -> bool {
        matches!(self, Self::Walk(_))
    }

    pub fn toggle(&mut self) {
        *self = match self {
            Self::Noclip(player) => Self::Walk(WalkPlayer::from_noclip(player)),
            Self::Walk(player) => Self::Noclip(player.to_noclip()),
        };
    }

    /// Velocity in units per second; noclip has none and reports zero.
    pub fn velocity(&self) -> [f32; 3] {
        match self {
            Self::Noclip(_) => [0.0; 3],
            Self::Walk(player) => player.state.velocity,
        }
    }

    /// Pose for rendering: eye position and view angles.
    pub fn view(&self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Noclip(player) => {
                let mut eye = player.position;
                eye[2] += NOCLIP_EYE_HEIGHT;
                (eye, player.view_angles)
            }
            Self::Walk(player) => (player.eye_position(), player.view_angles),
        }
    }

    fn step<W: PmoveWorld>(
        &mut self,
        world: &W,
        command: WalkCommand,
        touch: Option<&mut dyn PmoveTouch>,
    ) -> Result<(), StepError> {
        match self {
            Self::Noclip(player) => player.step(PlayerCommand {
                forward: command.forward,
                right: command.right,
                up: command.up,
                view_angles: command.view_angles,
            }),
            Self::Walk(player) => player.step_with_touch(world, command, touch),
        }
    }
}

/// Fixed 125 Hz accumulator over either movement mode, with optional map entities.
#[derive(Debug)]
pub struct ModeSimulation {
    mode: MovementMode,
    remainder: Duration,
    entities: Option<MapEntities>,
    ticks: i64,
    trigger_events: Vec<TriggerEvent>,
}

impl ModeSimulation {
    pub fn new(player: NoclipPlayer) -> Self {
        Self {
            mode: MovementMode::Noclip(player),
            remainder: Duration::ZERO,
            entities: None,
            ticks: 0,
            trigger_events: Vec::new(),
        }
    }

    /// Installs the map entities that act on the walking player.
    pub fn set_entities(&mut self, entities: MapEntities) {
        self.entities = Some(entities);
    }

    pub fn entities(&self) -> Option<&MapEntities> {
        self.entities.as_ref()
    }

    /// For supplying telefrag occupants or activating entities between ticks.
    pub fn entities_mut(&mut self) -> Option<&mut MapEntities> {
        self.entities.as_mut()
    }

    /// Takes the trigger events produced by walking ticks since the last call.
    pub fn drain_trigger_events(&mut self) -> Vec<TriggerEvent> {
        std::mem::take(&mut self.trigger_events)
    }

    pub fn mode(&self) -> &MovementMode {
        &self.mode
    }

    pub fn toggle_mode(&mut self) {
        self.mode.toggle();
    }

    pub fn advance_frame<W: PmoveWorld>(
        &mut self,
        world: &W,
        elapsed: Duration,
        command: WalkCommand,
    ) -> Result<usize, StepError> {
        self.advance(world, None::<&NoModels>, elapsed, command)
    }

    /// `advance_frame` with map triggers evaluated against `models` each tick.
    pub fn advance_frame_with_entities<W: PmoveWorld, M: BrushModels>(
        &mut self,
        world: &W,
        models: &M,
        elapsed: Duration,
        command: WalkCommand,
    ) -> Result<usize, StepError> {
        self.advance(world, Some(models), elapsed, command)
    }

    fn advance<W: PmoveWorld, M: BrushModels>(
        &mut self,
        world: &W,
        models: Option<&M>,
        elapsed: Duration,
        command: WalkCommand,
    ) -> Result<usize, StepError> {
        let accumulated = self
            .remainder
            .checked_add(elapsed)
            .ok_or(StepError::ElapsedTimeOverflow)?;
        let ticks = accumulated.as_nanos() / TICK_DURATION.as_nanos();
        let tick_count = usize::try_from(ticks).map_err(|_| StepError::TooManyTicks)?;
        for _ in 0..tick_count {
            self.ticks += 1;
            let time_ms = self.ticks * TICK_DURATION.as_millis() as i64;
            match (&mut self.entities, models) {
                (Some(entities), Some(models)) if self.mode.is_walking() => {
                    let mut touch = entities.player_touch(models, time_ms);
                    self.mode.step(world, command, Some(&mut touch))?;
                    let events = entities.drain_events();
                    if let MovementMode::Walk(player) = &mut self.mode
                        && events
                            .iter()
                            .any(|event| matches!(event, TriggerEvent::Teleport { .. }))
                    {
                        let angles = player.state.view_angles;
                        player.view_angles = [-angles[0], angles[1], angles[2]];
                    }
                    self.trigger_events.extend(events);
                }
                _ => {
                    self.mode.step(world, command, None)?;
                    if let Some(entities) = &mut self.entities {
                        entities.think(time_ms);
                        self.trigger_events.extend(entities.drain_events());
                    }
                }
            }
        }
        self.remainder =
            Duration::from_nanos((accumulated.as_nanos() % TICK_DURATION.as_nanos()) as u64);
        Ok(tick_count)
    }
}

/// Placeholder `BrushModels` for ticks without entities.
struct NoModels;

impl BrushModels for NoModels {
    fn model_bounds(&self, _model: usize) -> Option<([f32; 3], [f32; 3])> {
        None
    }
    fn box_touches_model(
        &self,
        _model: usize,
        _origin: [f32; 3],
        _mins: [f32; 3],
        _maxs: [f32; 3],
    ) -> bool {
        false
    }
}
