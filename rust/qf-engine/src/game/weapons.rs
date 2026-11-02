//! Weapon definitions: pure data, no state and no logic.
//!
//! Values are transcribed from the default (non-race) table of
//! `source/common/facilities/gs_weapondefs.cpp`. Each weapon has a single fire
//! mode: the legacy strong mode. The legacy weak mode and weak ammo are not ported.

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WeaponId {
    Gunblade,
    Machinegun,
    Riotgun,
    GrenadeLauncher,
    RocketLauncher,
    Plasmagun,
    Lasergun,
    Electrobolt,
    Shockwave,
    Instagun,
}

pub const WEAPON_COUNT: usize = 10;

impl WeaponId {
    pub const ALL: [WeaponId; WEAPON_COUNT] = [
        Self::Gunblade,
        Self::Machinegun,
        Self::Riotgun,
        Self::GrenadeLauncher,
        Self::RocketLauncher,
        Self::Plasmagun,
        Self::Lasergun,
        Self::Electrobolt,
        Self::Shockwave,
        Self::Instagun,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn def(self) -> &'static WeaponDef {
        &WEAPON_DEFS[self.index()]
    }

    /// Accepts the short name (`rl`), the full name (`rocketlauncher`, `gunblade`)
    /// and the item classname (`weapon_rocketlauncher`), case-insensitively.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let name = name.strip_prefix("weapon_").unwrap_or(&name);
        Self::ALL
            .into_iter()
            .find(|id| id.def().short_name == name || id.def().name == name)
    }
}

/// Immutable definition of one weapon. Times are in milliseconds, distances in
/// game units, speeds in units per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponDef {
    pub id: WeaponId,
    /// Lower-case identifier, also the item classname suffix (`weapon_<name>`).
    pub name: &'static str,
    pub short_name: &'static str,
    pub ammo_per_shot: u32,
    pub projectiles_per_shot: u32,
    pub switch_up_ms: u32,
    pub switch_down_ms: u32,
    /// Time between two shots (legacy `reload_time`).
    pub refire_ms: u32,
    /// Projectile lifetime, or the range of instant weapons.
    pub timeout: u32,
    pub smooth_refire: bool,
    pub damage: f32,
    pub self_damage_ratio: f32,
    pub knockback: f32,
    pub stun: f32,
    pub splash_radius: f32,
    pub splash_min_damage: f32,
    pub splash_min_knockback: f32,
    /// 0 for hitscan weapons.
    pub speed: f32,
    pub spread: f32,
    pub v_spread: f32,
    /// Ammo granted by a weapon or ammo pickup.
    pub ammo_pickup: u32,
    pub ammo_max: u32,
    pub low_ammo: u32,
}

impl WeaponDef {
    pub fn is_instant(&self) -> bool {
        self.speed == 0.0
    }

    /// Whether firing spawns a projectile entity.
    pub fn is_projectile(&self) -> bool {
        !self.is_instant()
    }
}

const UP_MS: u32 = 50;
const DOWN_MS: u32 = 50;
const LASERGUN_RANGE: u32 = 850;

#[allow(clippy::too_many_arguments)]
const fn def(
    id: WeaponId,
    name: &'static str,
    short_name: &'static str,
    shot: (u32, u32),
    timing: (u32, u32, u32, u32, bool),
    damage: [f32; 7],
    projectile: (f32, f32, f32),
    ammo: (u32, u32, u32),
) -> WeaponDef {
    WeaponDef {
        id,
        name,
        short_name,
        ammo_per_shot: shot.0,
        projectiles_per_shot: shot.1,
        switch_up_ms: timing.0,
        switch_down_ms: timing.1,
        refire_ms: timing.2,
        timeout: timing.3,
        smooth_refire: timing.4,
        damage: damage[0],
        self_damage_ratio: damage[1],
        knockback: damage[2],
        stun: damage[3],
        splash_radius: damage[4],
        splash_min_damage: damage[5],
        splash_min_knockback: damage[6],
        speed: projectile.0,
        spread: projectile.1,
        v_spread: projectile.2,
        ammo_pickup: ammo.0,
        ammo_max: ammo.1,
        low_ammo: ammo.2,
    }
}

use WeaponId::*;

/// Indexed by [`WeaponId::index`].
pub static WEAPON_DEFS: [WeaponDef; WEAPON_COUNT] = [
    def(
        Gunblade,
        "gunblade",
        "gb",
        // The gunblade is always owned and has no ammo: it is the free fire mode.
        (0, 1),
        (UP_MS, DOWN_MS, 600, 5000, false),
        [30.0, 1.0, 90.0, 0.0, 80.0, 8.0, 10.0],
        (2500.0, 0.0, 0.0),
        (0, 0, 0),
    ),
    def(
        Machinegun,
        "machinegun",
        "mg",
        (1, 1),
        (UP_MS, DOWN_MS, 50, 6000, false),
        [8.0, 0.0, 10.0, 50.0, 0.0, 0.0, 0.0],
        (0.0, 250.0, 250.0),
        (100, 100, 20),
    ),
    def(
        Riotgun,
        "riotgun",
        "rg",
        (1, 20),
        (UP_MS, DOWN_MS, 800, 8192, false),
        [5.0, 0.0, 7.0, 85.0, 0.0, 0.0, 0.0],
        (0.0, 160.0, 90.0),
        (10, 10, 3),
    ),
    def(
        GrenadeLauncher,
        "grenadelauncher",
        "gl",
        (1, 1),
        (UP_MS, DOWN_MS, 800, 1250, false),
        [80.0, 1.0, 100.0, 1250.0, 125.0, 15.0, 35.0],
        (1000.0, 0.0, 0.0),
        (10, 10, 3),
    ),
    def(
        RocketLauncher,
        "rocketlauncher",
        "rl",
        (1, 1),
        (UP_MS, DOWN_MS, 900, 10000, false),
        [90.0, 1.0, 100.0, 1250.0, 100.0, 10.0, 35.0],
        (1150.0, 0.0, 0.0),
        (10, 10, 3),
    ),
    def(
        Plasmagun,
        "plasmagun",
        "pg",
        (1, 1),
        (UP_MS, DOWN_MS, 100, 5000, false),
        [15.0, 0.5, 20.0, 200.0, 45.0, 5.0, 1.0],
        (2500.0, 0.0, 0.0),
        (100, 100, 20),
    ),
    def(
        Lasergun,
        "lasergun",
        "lg",
        (1, 1),
        (UP_MS, DOWN_MS, 50, LASERGUN_RANGE, true),
        [7.0, 0.0, 12.0, 300.0, 0.0, 0.0, 0.0],
        (0.0, 0.0, 0.0),
        (100, 100, 20),
    ),
    def(
        Electrobolt,
        "electrobolt",
        "eb",
        (1, 1),
        (UP_MS, DOWN_MS, 1250, 900, false),
        [70.0, 0.0, 80.0, 1000.0, 0.0, 75.0, 35.0],
        (0.0, 0.0, 0.0),
        (10, 10, 3),
    ),
    def(
        Shockwave,
        "shockwave",
        "sw",
        (1, 1),
        (300, DOWN_MS, 950, 10000, false),
        [55.0, 1.0, 90.0, 500.0, 90.0, 55.0, 35.0],
        (775.0, 0.0, 0.0),
        (10, 10, 3),
    ),
    def(
        Instagun,
        "instagun",
        "ig",
        (1, 1),
        (UP_MS, DOWN_MS, 1300, 8024, false),
        [200.0, 0.1, 95.0, 1000.0, 80.0, 0.0, 0.0],
        (0.0, 0.0, 0.0),
        (10, 10, 0),
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_indexed_by_id() {
        for id in WeaponId::ALL {
            assert_eq!(id.def().id, id);
        }
    }

    #[test]
    fn legacy_spot_values() {
        let rl = WeaponId::RocketLauncher.def();
        assert_eq!(
            (rl.damage, rl.knockback, rl.splash_radius),
            (90.0, 100.0, 100.0)
        );
        assert_eq!((rl.speed, rl.refire_ms, rl.ammo_max), (1150.0, 900, 10));
        let rg = WeaponId::Riotgun.def();
        assert_eq!(
            (rg.projectiles_per_shot, rg.damage, rg.spread),
            (20, 5.0, 160.0)
        );
        let mg = WeaponId::Machinegun.def();
        assert_eq!((mg.refire_ms, mg.ammo_pickup, mg.low_ammo), (50, 100, 20));
        assert!(WeaponId::Lasergun.def().smooth_refire);
        assert_eq!(WeaponId::Shockwave.def().switch_up_ms, 300);
    }

    #[test]
    fn hitscan_and_projectile_classification() {
        for id in [
            WeaponId::Machinegun,
            WeaponId::Riotgun,
            WeaponId::Lasergun,
            WeaponId::Electrobolt,
            WeaponId::Instagun,
        ] {
            assert!(id.def().is_instant() && !id.def().is_projectile(), "{id:?}");
        }
        for id in [
            WeaponId::Gunblade,
            WeaponId::GrenadeLauncher,
            WeaponId::RocketLauncher,
            WeaponId::Plasmagun,
            WeaponId::Shockwave,
        ] {
            assert!(id.def().is_projectile() && !id.def().is_instant(), "{id:?}");
        }
    }

    #[test]
    fn names_resolve() {
        assert_eq!(WeaponId::from_name("RL"), Some(WeaponId::RocketLauncher));
        assert_eq!(WeaponId::from_name("gunblade"), Some(WeaponId::Gunblade));
        assert_eq!(
            WeaponId::from_name("weapon_electrobolt"),
            Some(WeaponId::Electrobolt)
        );
        assert_eq!(WeaponId::from_name("nope"), None);
    }
}
