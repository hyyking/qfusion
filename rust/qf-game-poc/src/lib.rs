use std::fmt;
use std::time::Duration;

use bsp_poc::entities::Entity;

pub const TICK_RATE_HZ: u32 = 125;
pub const TICK_DURATION: Duration = Duration::from_millis(8);
pub const NOCLIP_SPEED: f32 = 320.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoclipPlayer {
    pub position: [f32; 3],
    /// Pitch, yaw, and roll in degrees.
    pub view_angles: [f32; 3],
}

impl NoclipPlayer {
    pub fn spawn(entities: &[Entity]) -> Result<Self, SpawnError> {
        let spawn = ["info_player_deathmatch", "info_player_start"]
            .into_iter()
            .find_map(|classname| {
                entities.iter().find_map(|entity| {
                    if !entity
                        .get("classname")
                        .is_some_and(|value| value.eq_ignore_ascii_case(classname))
                    {
                        return None;
                    }

                    let position = parse_vector(entity.get("origin")?)?;
                    let view_angles = parse_angles(entity)?;
                    Some((position, view_angles))
                })
            })
            .ok_or(SpawnError::NoSpawnPoint)?;

        Ok(Self {
            position: spawn.0,
            view_angles: normalize_angles(spawn.1),
        })
    }

    pub fn step(&mut self, command: PlayerCommand) -> Result<(), StepError> {
        command.validate()?;

        let view_angles = normalize_angles(command.view_angles);
        let pitch = view_angles[0].to_radians();
        let yaw = view_angles[1].to_radians();
        let (sin_pitch, cos_pitch) = pitch.sin_cos();
        let (sin_yaw, cos_yaw) = yaw.sin_cos();

        let forward = [cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch];
        let right = [sin_yaw, -cos_yaw, 0.0];
        let up = [-sin_pitch * cos_yaw, -sin_pitch * sin_yaw, cos_pitch];
        let axes = [
            command.forward.clamp(-1.0, 1.0),
            command.right.clamp(-1.0, 1.0),
            command.up.clamp(-1.0, 1.0),
        ];
        let mut wish = [0.0; 3];
        for axis in 0..3 {
            wish[axis] = forward[axis] * axes[0] + right[axis] * axes[1] + up[axis] * axes[2];
        }
        let length = dot(wish, wish).sqrt();
        if length > 0.0 {
            let distance = NOCLIP_SPEED * TICK_DURATION.as_secs_f32() / length.max(1.0);
            for (position, direction) in self.position.iter_mut().zip(wish) {
                *position += direction * distance;
            }
        }
        self.view_angles = view_angles;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlayerCommand {
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub view_angles: [f32; 3],
}

impl PlayerCommand {
    fn validate(self) -> Result<(), StepError> {
        if [self.forward, self.right, self.up]
            .into_iter()
            .chain(self.view_angles)
            .all(f32::is_finite)
        {
            Ok(())
        } else {
            Err(StepError::NonFiniteInput)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpawnError {
    NoSpawnPoint,
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSpawnPoint => write!(f, "map has no valid player spawn point"),
        }
    }
}

impl std::error::Error for SpawnError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepError {
    NonFiniteInput,
    ElapsedTimeOverflow,
    TooManyTicks,
}

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFiniteInput => write!(f, "player command contains a non-finite value"),
            Self::ElapsedTimeOverflow => write!(f, "elapsed frame time overflowed"),
            Self::TooManyTicks => write!(f, "frame time exceeds the supported tick count"),
        }
    }
}

impl std::error::Error for StepError {}

#[derive(Clone, Debug, PartialEq)]
pub struct FixedStepSimulation {
    player: NoclipPlayer,
    remainder: Duration,
}

impl FixedStepSimulation {
    pub fn new(player: NoclipPlayer) -> Self {
        Self {
            player,
            remainder: Duration::ZERO,
        }
    }

    pub fn player(&self) -> &NoclipPlayer {
        &self.player
    }

    pub fn advance_frame(
        &mut self,
        elapsed: Duration,
        command: PlayerCommand,
    ) -> Result<usize, StepError> {
        command.validate()?;
        let accumulated = self
            .remainder
            .checked_add(elapsed)
            .ok_or(StepError::ElapsedTimeOverflow)?;
        let ticks = accumulated.as_nanos() / TICK_DURATION.as_nanos();
        let tick_count = usize::try_from(ticks).map_err(|_| StepError::TooManyTicks)?;

        for _ in 0..tick_count {
            self.player.step(command)?;
        }
        self.remainder =
            Duration::from_nanos((accumulated.as_nanos() % TICK_DURATION.as_nanos()) as u64);
        Ok(tick_count)
    }
}

fn parse_vector(value: &str) -> Option<[f32; 3]> {
    let values: Vec<_> = value
        .split_ascii_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<_, _>>()
        .ok()?;
    let vector: [f32; 3] = values.try_into().ok()?;
    vector
        .iter()
        .all(|value| value.is_finite())
        .then_some(vector)
}

fn parse_angles(entity: &Entity) -> Option<[f32; 3]> {
    if let Some(angles) = entity.get("angles") {
        return parse_vector(angles);
    }
    let yaw = entity.get("angle").unwrap_or("0").parse::<f32>().ok()?;
    yaw.is_finite().then_some([0.0, yaw, 0.0])
}

fn normalize_angles(mut angles: [f32; 3]) -> [f32; 3] {
    angles[0] = angles[0].clamp(-89.0, 89.0);
    angles[1] = angles[1].rem_euclid(360.0);
    angles[2] = 0.0;
    angles
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(classname: &str, origin: &str, angles: Option<(&str, &str)>) -> Entity {
        let mut properties = vec![
            ("classname".into(), classname.into()),
            ("origin".into(), origin.into()),
        ];
        if let Some((key, value)) = angles {
            properties.push((key.into(), value.into()));
        }
        Entity { properties }
    }

    fn test_player() -> NoclipPlayer {
        NoclipPlayer {
            position: [0.0; 3],
            view_angles: [0.0; 3],
        }
    }

    #[test]
    fn selects_deathmatch_spawn_before_start_and_reads_angles() {
        let entities = [
            entity("info_player_start", "1 2 3", Some(("angle", "90"))),
            entity(
                "info_player_deathmatch",
                "10 20 30",
                Some(("angles", "15 450 20")),
            ),
        ];
        let player = NoclipPlayer::spawn(&entities).unwrap();
        assert_eq!(player.position, [10.0, 20.0, 30.0]);
        assert_eq!(player.view_angles, [15.0, 90.0, 0.0]);
    }

    #[test]
    fn falls_back_to_start_and_single_angle_is_yaw() {
        let entities = [entity(
            "info_player_start",
            "-5 4 9",
            Some(("angle", "-90")),
        )];
        let player = NoclipPlayer::spawn(&entities).unwrap();
        assert_eq!(player.position, [-5.0, 4.0, 9.0]);
        assert_eq!(player.view_angles, [0.0, 270.0, 0.0]);
    }

    #[test]
    fn ignores_invalid_spawn_entities_and_reports_missing_spawn() {
        let entities = [
            entity("worldspawn", "0 0 0", None),
            entity("info_player_deathmatch", "bad origin", None),
        ];
        assert_eq!(
            NoclipPlayer::spawn(&entities),
            Err(SpawnError::NoSpawnPoint)
        );
    }

    #[test]
    fn noclip_moves_forward_at_fixed_speed_without_collision() {
        let mut player = test_player();
        let command = PlayerCommand {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..TICK_RATE_HZ {
            player.step(command).unwrap();
        }
        assert!((player.position[0] - NOCLIP_SPEED).abs() < 0.001);
        assert!(player.position[1].abs() < 0.001);
        assert!(player.position[2].abs() < 0.001);
        player
            .step(PlayerCommand {
                up: 1.0,
                ..Default::default()
            })
            .unwrap();
        assert!((player.position[2] - NOCLIP_SPEED / TICK_RATE_HZ as f32).abs() < 1e-5);
    }

    #[test]
    fn view_angles_direct_movement_and_diagonal_speed() {
        let mut player = test_player();
        player
            .step(PlayerCommand {
                forward: 1.0,
                view_angles: [0.0, 90.0, 0.0],
                ..Default::default()
            })
            .unwrap();
        assert!(player.position[0].abs() < 1e-5);
        assert!((player.position[1] - NOCLIP_SPEED / TICK_RATE_HZ as f32).abs() < 1e-5);
        assert_eq!(player.view_angles, [0.0, 90.0, 0.0]);

        let mut diagonal = test_player();
        diagonal
            .step(PlayerCommand {
                forward: 1.0,
                right: 1.0,
                ..Default::default()
            })
            .unwrap();
        assert!(
            (dot(diagonal.position, diagonal.position).sqrt() - NOCLIP_SPEED / TICK_RATE_HZ as f32)
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn fixed_ticks_are_independent_of_render_frame_grouping() {
        let player = test_player();
        let command = PlayerCommand {
            forward: 1.0,
            right: 0.25,
            view_angles: [5.0, 30.0, 0.0],
            ..Default::default()
        };
        let mut thirty_hz = FixedStepSimulation::new(player);
        let mut one_forty_four_hz = FixedStepSimulation::new(player);
        for _ in 0..29 {
            thirty_hz
                .advance_frame(Duration::from_nanos(33_333_333), command)
                .unwrap();
        }
        thirty_hz
            .advance_frame(Duration::from_nanos(33_333_343), command)
            .unwrap();
        for _ in 0..143 {
            one_forty_four_hz
                .advance_frame(Duration::from_nanos(6_944_444), command)
                .unwrap();
        }
        one_forty_four_hz
            .advance_frame(Duration::from_nanos(6_944_508), command)
            .unwrap();

        assert_eq!(thirty_hz, one_forty_four_hz);
        assert!(
            (dot(thirty_hz.player.position, thirty_hz.player.position).sqrt() - 320.0).abs() < 0.01
        );
    }

    #[test]
    fn rejects_non_finite_commands_without_changing_player() {
        let mut player = test_player();
        let before = player;
        assert_eq!(
            player.step(PlayerCommand {
                forward: f32::NAN,
                ..Default::default()
            }),
            Err(StepError::NonFiniteInput)
        );
        assert_eq!(player, before);
    }
}
