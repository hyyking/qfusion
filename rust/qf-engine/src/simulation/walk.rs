use std::time::Duration;

use super::player::{NoclipPlayer, PlayerCommand, StepError, TICK_DURATION};
use super::pmove::{
    BUTTON_SPECIAL, PmType, PmoveCommand, PmoveRules, PmoveState, PmoveWorld, angle_to_short, pmove,
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
        eye[2] += self.state.view_height as f32;
        eye
    }

    pub fn step<W: PmoveWorld>(
        &mut self,
        world: &W,
        command: WalkCommand,
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
        pmove(world, &self.rules, &mut self.state, &pm_command);
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

    fn step<W: PmoveWorld>(&mut self, world: &W, command: WalkCommand) -> Result<(), StepError> {
        match self {
            Self::Noclip(player) => player.step(PlayerCommand {
                forward: command.forward,
                right: command.right,
                up: command.up,
                view_angles: command.view_angles,
            }),
            Self::Walk(player) => player.step(world, command),
        }
    }
}

/// Fixed 125 Hz accumulator over either movement mode.
#[derive(Clone, Debug)]
pub struct ModeSimulation {
    mode: MovementMode,
    remainder: Duration,
}

impl ModeSimulation {
    pub fn new(player: NoclipPlayer) -> Self {
        Self {
            mode: MovementMode::Noclip(player),
            remainder: Duration::ZERO,
        }
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
        let accumulated = self
            .remainder
            .checked_add(elapsed)
            .ok_or(StepError::ElapsedTimeOverflow)?;
        let ticks = accumulated.as_nanos() / TICK_DURATION.as_nanos();
        let tick_count = usize::try_from(ticks).map_err(|_| StepError::TooManyTicks)?;
        for _ in 0..tick_count {
            self.mode.step(world, command)?;
        }
        self.remainder =
            Duration::from_nanos((accumulated.as_nanos() % TICK_DURATION.as_nanos()) as u64);
        Ok(tick_count)
    }
}
