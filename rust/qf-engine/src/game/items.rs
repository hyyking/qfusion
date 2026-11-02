//! Item definitions: pure data, no state and no logic.
//!
//! Values follow `source/common/facilities/gs_items.cpp` and the default respawn
//! times of `g_gametypes.cpp`. Weapons have a single ammo type, so ammo items map
//! one to one onto weapons and the legacy `_weak` ammo and ammo packs are not ported.

use super::weapons::WeaponId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Powerup {
    Quad,
    Warshell,
    Regen,
}

pub const POWERUP_COUNT: usize = 3;

impl Powerup {
    pub fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemKind {
    Weapon(WeaponId),
    Ammo(WeaponId),
    Armor,
    /// `ignore_max` items (small, mega, ultra) may raise health above the maximum.
    Health {
        ignore_max: bool,
    },
    Powerup(Powerup),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ItemId(pub usize);

impl ItemId {
    pub fn def(self) -> &'static ItemDef {
        &ITEM_DEFS[self.0]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemDef {
    pub classname: &'static str,
    pub name: &'static str,
    pub kind: ItemKind,
    /// Health, armor or powerup seconds granted; weapon and ammo items grant the
    /// ammo of their weapon definition instead.
    pub quantity: u32,
    /// Armor ceiling of the item (`inventory_max`), 0 when it does not apply.
    pub max: u32,
    pub respawn_ms: u32,
}

impl ItemDef {
    pub fn amount(&self) -> u32 {
        match self.kind {
            ItemKind::Weapon(weapon) | ItemKind::Ammo(weapon) => weapon.def().ammo_pickup,
            _ => self.quantity,
        }
    }
}

pub const WEAPON_RESPAWN_MS: u32 = 5_000;
pub const AMMO_RESPAWN_MS: u32 = 20_000;
pub const ARMOR_RESPAWN_MS: u32 = 25_000;
pub const HEALTH_RESPAWN_MS: u32 = 15_000;
pub const MEGAHEALTH_RESPAWN_MS: u32 = 20_000;
pub const ULTRAHEALTH_RESPAWN_MS: u32 = 40_000;
pub const POWERUP_RESPAWN_MS: u32 = 90_000;

pub const ARMOR_GA_MAX: u32 = 100;
pub const ARMOR_YA_MAX: u32 = 125;
pub const ARMOR_RA_MAX: u32 = 150;
pub const ARMOR_SHARD_MAX: u32 = 200;
pub const POWERUP_SECONDS: u32 = 30;

const fn weapon(id: WeaponId, classname: &'static str, name: &'static str) -> ItemDef {
    ItemDef {
        classname,
        name,
        kind: ItemKind::Weapon(id),
        quantity: 0,
        max: 0,
        respawn_ms: WEAPON_RESPAWN_MS,
    }
}

const fn ammo(id: WeaponId, classname: &'static str, name: &'static str) -> ItemDef {
    ItemDef {
        classname,
        name,
        kind: ItemKind::Ammo(id),
        quantity: 0,
        max: 0,
        respawn_ms: AMMO_RESPAWN_MS,
    }
}

const fn armor(classname: &'static str, name: &'static str, quantity: u32, max: u32) -> ItemDef {
    ItemDef {
        classname,
        name,
        kind: ItemKind::Armor,
        quantity,
        max,
        respawn_ms: ARMOR_RESPAWN_MS,
    }
}

const fn health(
    classname: &'static str,
    name: &'static str,
    quantity: u32,
    ignore_max: bool,
    respawn_ms: u32,
) -> ItemDef {
    ItemDef {
        classname,
        name,
        kind: ItemKind::Health { ignore_max },
        quantity,
        max: 0,
        respawn_ms,
    }
}

const fn powerup(classname: &'static str, name: &'static str, kind: Powerup) -> ItemDef {
    ItemDef {
        classname,
        name,
        kind: ItemKind::Powerup(kind),
        quantity: POWERUP_SECONDS,
        max: 0,
        respawn_ms: POWERUP_RESPAWN_MS,
    }
}

use WeaponId::*;

/// Every item that can be placed in a map. The gunblade is always owned and never
/// in the world, so it has no item.
pub static ITEM_DEFS: [ItemDef; 30] = [
    weapon(Machinegun, "weapon_machinegun", "Machinegun"),
    weapon(Riotgun, "weapon_riotgun", "Riotgun"),
    weapon(
        GrenadeLauncher,
        "weapon_grenadelauncher",
        "Grenade Launcher",
    ),
    weapon(RocketLauncher, "weapon_rocketlauncher", "Rocket Launcher"),
    weapon(Plasmagun, "weapon_plasmagun", "Plasmagun"),
    weapon(Lasergun, "weapon_lasergun", "Lasergun"),
    weapon(Electrobolt, "weapon_electrobolt", "Electrobolt"),
    weapon(Shockwave, "weapon_shockwave", "Shockwave"),
    weapon(Instagun, "weapon_instagun", "Instagun"),
    ammo(Machinegun, "ammo_machinegun", "Bullets"),
    ammo(Riotgun, "ammo_riotgun", "Shells"),
    ammo(GrenadeLauncher, "ammo_grenadelauncher", "Grenades"),
    ammo(RocketLauncher, "ammo_rocketlauncher", "Rockets"),
    ammo(Plasmagun, "ammo_plasmagun", "Plasma"),
    ammo(Lasergun, "ammo_lasergun", "Lasers"),
    ammo(Electrobolt, "ammo_electrobolt", "Bolts"),
    ammo(Shockwave, "ammo_shockwave", "Waves"),
    ammo(Instagun, "ammo_instagun", "Instas"),
    armor("item_armor_ga", "Green Armor", 50, ARMOR_GA_MAX),
    armor("item_armor_ya", "Yellow Armor", 75, ARMOR_YA_MAX),
    armor("item_armor_ra", "Red Armor", 100, ARMOR_RA_MAX),
    armor("item_armor_shard", "Armor Shard", 5, ARMOR_SHARD_MAX),
    health("item_health_small", "5 Health", 5, true, HEALTH_RESPAWN_MS),
    health(
        "item_health_medium",
        "25 Health",
        25,
        false,
        HEALTH_RESPAWN_MS,
    ),
    health(
        "item_health_large",
        "50 Health",
        50,
        false,
        HEALTH_RESPAWN_MS,
    ),
    health(
        "item_health_mega",
        "Mega Health",
        100,
        true,
        MEGAHEALTH_RESPAWN_MS,
    ),
    health(
        "item_health_ultra",
        "Ultra Health",
        100,
        true,
        ULTRAHEALTH_RESPAWN_MS,
    ),
    powerup("item_quad", "Quad Damage", Powerup::Quad),
    powerup("item_warshell", "WarShell", Powerup::Warshell),
    powerup("item_regen", "Regeneration", Powerup::Regen),
];

/// Looks an item up by map classname, case-insensitively.
pub fn item_by_classname(classname: &str) -> Option<ItemId> {
    ITEM_DEFS
        .iter()
        .position(|def| def.classname.eq_ignore_ascii_case(classname))
        .map(ItemId)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classnames_are_unique_and_resolve() {
        for (index, def) in ITEM_DEFS.iter().enumerate() {
            assert_eq!(item_by_classname(def.classname), Some(ItemId(index)));
        }
        assert_eq!(
            item_by_classname("ITEM_QUAD").map(|i| i.def().name),
            Some("Quad Damage")
        );
        assert_eq!(item_by_classname("ammo_rocketlauncher_weak"), None);
        assert_eq!(item_by_classname("weapon_gunblade"), None);
    }

    #[test]
    fn legacy_quantities_and_respawns() {
        let ga = item_by_classname("item_armor_ga").unwrap().def();
        assert_eq!((ga.quantity, ga.max, ga.respawn_ms), (50, 100, 25_000));
        let rl = item_by_classname("weapon_rocketlauncher").unwrap().def();
        assert_eq!((rl.amount(), rl.respawn_ms), (10, 5_000));
        let mg = item_by_classname("ammo_machinegun").unwrap().def();
        assert_eq!((mg.amount(), mg.respawn_ms), (100, 20_000));
        let mega = item_by_classname("item_health_mega").unwrap().def();
        assert_eq!(mega.respawn_ms, 20_000);
        assert_eq!(
            item_by_classname("item_quad").unwrap().def().respawn_ms,
            90_000
        );
    }
}
