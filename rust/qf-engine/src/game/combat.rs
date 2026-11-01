use std::collections::BTreeMap;

use crate::simulation::pmove::Vec3;

/// Armor absorbs this fraction of incoming damage and degrades by the same amount
/// (`ARMOR_PROTECTION`/`ARMOR_DEGRADATION` in gs_public.h).
pub const ARMOR_FRACTION: f32 = 0.66;
pub const START_HEALTH: f32 = 100.0;
pub const START_ARMOR: f32 = 100.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Team {
    Alpha,
    Beta,
}

impl Team {
    pub fn other(self) -> Self {
        match self {
            Self::Alpha => Self::Beta,
            Self::Beta => Self::Alpha,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Alpha => 0,
            Self::Beta => 1,
        }
    }

    pub fn from_index(index: i64) -> Option<Self> {
        match index {
            0 => Some(Self::Alpha),
            1 => Some(Self::Beta),
            _ => None,
        }
    }
}

/// Engine-owned player state. Which rules apply to it (rounds, roles, scoring) is up
/// to the gametype script; weapons are inventory data only, firing is not simulated.
#[derive(Clone, Debug)]
pub struct Player {
    pub team: Team,
    pub alive: bool,
    pub health: f32,
    pub armor: f32,
    pub origin: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub weapons: Vec<(String, u32)>,
    /// Free-form per-player counters and flags owned by the gametype script.
    pub stats: BTreeMap<String, i64>,
}

impl Player {
    pub fn new(team: Team) -> Self {
        Self {
            team,
            alive: false,
            health: 0.0,
            armor: 0.0,
            origin: [0.0; 3],
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            weapons: Vec::new(),
            stats: BTreeMap::new(),
        }
    }

    pub fn give_inventory(&mut self) {
        self.health = START_HEALTH;
        self.armor = START_ARMOR;
    }

    pub fn forward(&self) -> Vec3 {
        let (yaw, pitch) = (self.yaw.to_radians(), self.pitch.to_radians());
        [
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            -pitch.sin(),
        ]
    }

    /// Applies damage through armor and returns `true` if the player died.
    pub fn take_damage(&mut self, damage: f32) -> bool {
        if !self.alive || damage <= 0.0 {
            return false;
        }
        let max_save = damage.min(self.armor / ARMOR_FRACTION);
        let save = max_save * ARMOR_FRACTION;
        self.armor = (self.armor - save).max(0.0);
        self.health -= damage - save;
        if self.health <= 0.0 {
            self.health = 0.0;
            self.alive = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armor_absorbs_two_thirds_and_degrades() {
        let mut player = Player::new(Team::Alpha);
        player.alive = true;
        player.give_inventory();
        assert!(!player.take_damage(30.0));
        assert!((player.health - (100.0 - 30.0 * 0.34)).abs() < 1e-3);
        assert!((player.armor - (100.0 - 30.0 * 0.66)).abs() < 1e-3);
    }

    #[test]
    fn empty_armor_passes_damage_through() {
        let mut player = Player::new(Team::Beta);
        player.alive = true;
        player.health = 50.0;
        assert!(player.take_damage(50.0));
        assert!(!player.alive);
        assert_eq!(player.health, 0.0);
    }

    #[test]
    fn dead_players_take_no_damage() {
        let mut player = Player::new(Team::Beta);
        assert!(!player.take_damage(500.0));
    }

    #[test]
    fn forward_follows_yaw_and_pitch() {
        let mut player = Player::new(Team::Alpha);
        player.yaw = 90.0;
        let forward = player.forward();
        assert!(forward[0].abs() < 1e-5 && (forward[1] - 1.0).abs() < 1e-5);
        player.pitch = 90.0;
        assert!((player.forward()[2] + 1.0).abs() < 1e-5);
    }
}
