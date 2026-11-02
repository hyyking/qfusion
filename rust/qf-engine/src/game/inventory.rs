//! Player weapon inventory and the weapon switching state machine.
//!
//! Pure state: no globals, no time source. The owner advances it with
//! [`Inventory::tick`] once per fixed simulation tick and drives it with
//! [`Inventory::request_select`]; firing (PR 2) reuses the same state and timer.

use super::items::{POWERUP_COUNT, Powerup};
use super::weapons::{WEAPON_COUNT, WeaponId};

/// Weapon selection order of `GS_SelectBestWeapon`-style auto switching.
pub const BEST_WEAPON_ORDER: [WeaponId; WEAPON_COUNT] = [
    WeaponId::Instagun,
    WeaponId::Electrobolt,
    WeaponId::RocketLauncher,
    WeaponId::Lasergun,
    WeaponId::Plasmagun,
    WeaponId::GrenadeLauncher,
    WeaponId::Shockwave,
    WeaponId::Riotgun,
    WeaponId::Machinegun,
    WeaponId::Gunblade,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WeaponState {
    Ready,
    Firing,
    Reloading,
    SwitchingUp,
    SwitchingDown,
}

#[derive(Clone, Debug, PartialEq, bevy_ecs::component::Component)]
pub struct Inventory {
    owned: [bool; WEAPON_COUNT],
    ammo: [u32; WEAPON_COUNT],
    powerups: [u32; POWERUP_COUNT],
    selected: Option<WeaponId>,
    pending: Option<WeaponId>,
    state: WeaponState,
    state_ms: u32,
}

impl Default for Inventory {
    fn default() -> Self {
        Self {
            owned: [false; WEAPON_COUNT],
            ammo: [0; WEAPON_COUNT],
            powerups: [0; POWERUP_COUNT],
            selected: None,
            pending: None,
            state: WeaponState::Ready,
            state_ms: 0,
        }
    }
}

impl Inventory {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn owns(&self, weapon: WeaponId) -> bool {
        self.owned[weapon.index()]
    }

    /// Owned weapons with their ammo, in weapon id order.
    pub fn owned(&self) -> impl Iterator<Item = (WeaponId, u32)> + '_ {
        WeaponId::ALL
            .into_iter()
            .filter(|&weapon| self.owns(weapon))
            .map(|weapon| (weapon, self.ammo(weapon)))
    }

    pub fn ammo(&self, weapon: WeaponId) -> u32 {
        self.ammo[weapon.index()]
    }

    /// Sets ammo exactly, without the pickup ceiling (gametype loadouts).
    pub fn set_ammo(&mut self, weapon: WeaponId, amount: u32) {
        self.ammo[weapon.index()] = amount;
    }

    /// Adds ammo up to the weapon's maximum. Returns false when already full or
    /// the weapon takes no ammo.
    pub fn add_ammo(&mut self, weapon: WeaponId, amount: u32) -> bool {
        let max = weapon.def().ammo_max;
        let current = &mut self.ammo[weapon.index()];
        if *current >= max {
            return false;
        }
        *current = (*current + amount).min(max);
        true
    }

    /// Whether a shot can be paid for. Weapons without an ammo cost always can.
    pub fn has_ammo(&self, weapon: WeaponId) -> bool {
        self.ammo(weapon) >= weapon.def().ammo_per_shot
    }

    /// Consumes the ammo of one shot; false (and nothing consumed) when short.
    pub fn spend_shot(&mut self, weapon: WeaponId) -> bool {
        let cost = weapon.def().ammo_per_shot;
        if self.ammo(weapon) < cost {
            return false;
        }
        self.ammo[weapon.index()] -= cost;
        true
    }

    /// Gives the weapon with `ammo` (exact). Returns whether it was newly owned.
    /// The first weapon given becomes the selected one.
    pub fn give_weapon(&mut self, weapon: WeaponId, ammo: u32) -> bool {
        let new = !self.owned[weapon.index()];
        self.owned[weapon.index()] = true;
        self.ammo[weapon.index()] = ammo;
        if self.selected.is_none() {
            self.begin_up(weapon);
        }
        new
    }

    /// Picks up a weapon item: owns it and adds its ammo. Returns false when the
    /// weapon is owned and its ammo is full (nothing to gain).
    pub fn pickup_weapon(&mut self, weapon: WeaponId) -> bool {
        let new = !self.owned[weapon.index()];
        let gained = self.add_ammo(weapon, weapon.def().ammo_pickup);
        if !new && !gained {
            return false;
        }
        self.owned[weapon.index()] = true;
        if self.selected.is_none() {
            self.begin_up(weapon);
        }
        true
    }

    pub fn powerup_seconds(&self, powerup: Powerup) -> u32 {
        self.powerups[powerup.index()]
    }

    pub fn add_powerup(&mut self, powerup: Powerup, seconds: u32) {
        self.powerups[powerup.index()] += seconds;
    }

    pub fn selected(&self) -> Option<WeaponId> {
        self.selected
    }

    pub fn pending(&self) -> Option<WeaponId> {
        self.pending
    }

    pub fn state(&self) -> WeaponState {
        self.state
    }

    pub fn state_remaining_ms(&self) -> u32 {
        self.state_ms
    }

    /// The weapon the player is, or is about to be, holding.
    pub fn target(&self) -> Option<WeaponId> {
        self.pending.or(self.selected)
    }

    /// Whether the selected weapon may start a shot now.
    pub fn can_fire(&self) -> bool {
        self.state == WeaponState::Ready
            && self.selected.is_some_and(|weapon| self.has_ammo(weapon))
    }

    /// Enters Firing for the selected weapon's refire time after a shot.
    pub fn begin_refire(&mut self) {
        if let Some(weapon) = self.selected {
            self.state = WeaponState::Firing;
            self.state_ms = weapon.def().refire_ms;
        }
    }

    fn begin_up(&mut self, weapon: WeaponId) {
        self.selected = Some(weapon);
        self.pending = None;
        self.state = WeaponState::SwitchingUp;
        self.state_ms = weapon.def().switch_up_ms;
    }

    /// Asks to switch to `weapon`. Returns false when it is not owned or already held.
    pub fn request_select(&mut self, weapon: WeaponId) -> bool {
        if !self.owns(weapon) || self.target() == Some(weapon) {
            return false;
        }
        match self.selected {
            None => self.begin_up(weapon),
            Some(current) => {
                self.pending = Some(weapon);
                if self.state != WeaponState::SwitchingDown {
                    self.state = WeaponState::SwitchingDown;
                    self.state_ms = current.def().switch_down_ms;
                }
            }
        }
        true
    }

    /// Next owned weapon with ammo after the one being held, in id order, wrapping.
    /// `step` is +1 for scrolling up and -1 for scrolling down.
    pub fn cycle(&self, step: i32) -> Option<WeaponId> {
        let current = self.target()?.index() as i32;
        let count = WEAPON_COUNT as i32;
        (1..count)
            .map(|offset| {
                WeaponId::ALL[(current + step.signum() * offset).rem_euclid(count) as usize]
            })
            .find(|&weapon| self.owns(weapon) && self.has_ammo(weapon))
    }

    /// Best owned weapon with ammo, see [`BEST_WEAPON_ORDER`].
    pub fn best_weapon(&self) -> Option<WeaponId> {
        BEST_WEAPON_ORDER
            .into_iter()
            .find(|&weapon| self.owns(weapon) && self.has_ammo(weapon))
    }

    /// Switches to the best weapon, used when the held one runs dry.
    pub fn select_best(&mut self) -> bool {
        self.best_weapon()
            .is_some_and(|weapon| self.request_select(weapon))
    }

    /// Advances the weapon timer by one simulation step.
    pub fn tick(&mut self, dt_ms: u32) {
        if self.state == WeaponState::Ready {
            return;
        }
        self.state_ms = self.state_ms.saturating_sub(dt_ms);
        if self.state_ms > 0 {
            return;
        }
        match self.state {
            WeaponState::SwitchingDown => {
                let next = self.pending.take().filter(|&weapon| self.owns(weapon));
                match next.or_else(|| self.best_weapon()) {
                    Some(weapon) => self.begin_up(weapon),
                    None => {
                        self.selected = None;
                        self.state = WeaponState::Ready;
                    }
                }
            }
            _ => self.state = WeaponState::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use WeaponId::*;

    fn armed() -> Inventory {
        let mut inv = Inventory::default();
        inv.give_weapon(Gunblade, 0);
        inv.give_weapon(RocketLauncher, 5);
        inv.give_weapon(Machinegun, 0);
        inv
    }

    fn run(inv: &mut Inventory, ms: u32) {
        for _ in 0..ms / 8 + 1 {
            inv.tick(8);
        }
    }

    #[test]
    fn first_weapon_is_raised() {
        let mut inv = armed();
        assert_eq!(inv.selected(), Some(Gunblade));
        assert_eq!(inv.state(), WeaponState::SwitchingUp);
        assert!(!inv.can_fire());
        run(&mut inv, 50);
        assert_eq!(inv.state(), WeaponState::Ready);
        assert!(inv.can_fire(), "gunblade costs no ammo");
    }

    #[test]
    fn switching_goes_down_then_up() {
        let mut inv = armed();
        run(&mut inv, 50);
        assert!(inv.request_select(RocketLauncher));
        assert_eq!(inv.state(), WeaponState::SwitchingDown);
        assert_eq!(inv.selected(), Some(Gunblade));
        assert_eq!(inv.pending(), Some(RocketLauncher));
        run(&mut inv, 50);
        assert_eq!(inv.state(), WeaponState::SwitchingUp);
        assert_eq!(inv.selected(), Some(RocketLauncher));
        run(&mut inv, 50);
        assert_eq!((inv.state(), inv.pending()), (WeaponState::Ready, None));
    }

    #[test]
    fn select_rejects_unowned_and_current() {
        let mut inv = armed();
        run(&mut inv, 50);
        assert!(!inv.request_select(Lasergun));
        assert!(!inv.request_select(Gunblade));
    }

    #[test]
    fn reselecting_during_switch_changes_target_without_restarting() {
        let mut inv = armed();
        run(&mut inv, 50);
        inv.request_select(RocketLauncher);
        inv.tick(8);
        let left = inv.state_remaining_ms();
        assert!(inv.request_select(Machinegun));
        assert_eq!(inv.state_remaining_ms(), left);
        run(&mut inv, 50);
        assert_eq!(inv.selected(), Some(Machinegun));
    }

    #[test]
    fn ammo_is_capped_and_shots_are_paid() {
        let mut inv = armed();
        assert!(inv.add_ammo(RocketLauncher, 100));
        assert_eq!(inv.ammo(RocketLauncher), 10);
        assert!(!inv.add_ammo(RocketLauncher, 1));
        assert!(inv.spend_shot(RocketLauncher));
        assert_eq!(inv.ammo(RocketLauncher), 9);
        assert!(!inv.spend_shot(Machinegun));
        assert_eq!(inv.ammo(Machinegun), 0);
    }

    #[test]
    fn loadout_ammo_may_exceed_pickup_cap() {
        let mut inv = Inventory::default();
        inv.give_weapon(Lasergun, 180);
        assert_eq!(inv.ammo(Lasergun), 180);
    }

    #[test]
    fn pickup_weapon_adds_default_ammo_once_full() {
        let mut inv = Inventory::default();
        assert!(inv.pickup_weapon(RocketLauncher));
        assert_eq!(inv.ammo(RocketLauncher), 10);
        assert!(!inv.pickup_weapon(RocketLauncher), "owned and full");
        inv.spend_shot(RocketLauncher);
        assert!(inv.pickup_weapon(RocketLauncher));
        assert_eq!(inv.ammo(RocketLauncher), 10);
    }

    #[test]
    fn cycle_skips_empty_and_unowned() {
        let inv = armed();
        assert_eq!(inv.cycle(1), Some(RocketLauncher));
        assert_eq!(
            inv.cycle(-1),
            Some(RocketLauncher),
            "machinegun has no ammo"
        );
        let mut inv = inv;
        inv.set_ammo(Machinegun, 10);
        assert_eq!(inv.cycle(1), Some(Machinegun));
        assert_eq!(inv.cycle(-1), Some(RocketLauncher));
    }

    #[test]
    fn best_weapon_prefers_stronger_with_ammo() {
        let mut inv = armed();
        assert_eq!(inv.best_weapon(), Some(RocketLauncher));
        inv.set_ammo(RocketLauncher, 0);
        assert_eq!(inv.best_weapon(), Some(Gunblade));
        assert!(inv.select_best() || inv.selected() == Some(Gunblade));
    }

    #[test]
    fn refire_returns_to_ready() {
        let mut inv = armed();
        run(&mut inv, 50);
        inv.begin_refire();
        assert_eq!(inv.state(), WeaponState::Firing);
        assert_eq!(inv.state_remaining_ms(), 600);
        run(&mut inv, 600);
        assert!(inv.can_fire());
    }
}
