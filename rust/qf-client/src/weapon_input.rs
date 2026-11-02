//! Turns fire, number keys and wheel steps into the engine's per-tick [`PlayerInput`].
//!
//! Fire is level-triggered (held). Select and cycle are one-shot: [`WeaponInput::take`]
//! hands them over once and clears them, so a keypress that lands between ticks is not lost
//! and is not applied twice.

use qf_engine::game::{PlayerInput, WeaponId};

#[derive(Debug, Default)]
pub struct WeaponInput {
    fire: bool,
    select: Option<WeaponId>,
    cycle: i32,
}

impl WeaponInput {
    pub fn set_fire(&mut self, held: bool) {
        self.fire = held;
    }

    /// `1`..`9` select the weapons in `WeaponId::ALL` order and `0` the tenth.
    pub fn weapon_for_digit(digit: char) -> Option<WeaponId> {
        let index = match digit {
            '1'..='9' => digit as usize - '1' as usize,
            '0' => 9,
            _ => return None,
        };
        WeaponId::ALL.get(index).copied()
    }

    /// Returns whether `typed` was a weapon key.
    pub fn select_key(&mut self, typed: &str) -> bool {
        let mut chars = typed.chars();
        let weapon = match (chars.next(), chars.next()) {
            (Some(c), None) => Self::weapon_for_digit(c),
            _ => None,
        };
        if weapon.is_some() {
            self.select = weapon;
            self.cycle = 0;
        }
        weapon.is_some()
    }

    /// +1 selects the next weapon, -1 the previous one.
    pub fn cycle(&mut self, steps: i32) {
        self.cycle = self.cycle.saturating_add(steps);
        self.select = None;
    }

    /// Drops held and pending input, for menus and lost focus.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The input for the next tick; select and cycle are consumed.
    pub fn take(&mut self) -> PlayerInput {
        PlayerInput {
            fire: self.fire,
            select: self.select.take(),
            cycle: std::mem::take(&mut self.cycle),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_map_to_weapons_in_order() {
        assert_eq!(WeaponInput::weapon_for_digit('1'), Some(WeaponId::Gunblade));
        assert_eq!(
            WeaponInput::weapon_for_digit('5'),
            Some(WeaponId::RocketLauncher)
        );
        assert_eq!(WeaponInput::weapon_for_digit('0'), Some(WeaponId::Instagun));
        assert_eq!(WeaponInput::weapon_for_digit('a'), None);
    }

    #[test]
    fn select_and_cycle_apply_once_but_fire_stays_held() {
        let mut input = WeaponInput::default();
        input.set_fire(true);
        assert!(input.select_key("2"));
        let first = input.take();
        assert!(first.fire);
        assert_eq!(first.select, Some(WeaponId::Machinegun));
        let second = input.take();
        assert!(second.fire);
        assert_eq!(second.select, None);
        assert_eq!(second.cycle, 0);
    }

    #[test]
    fn cycle_accumulates_and_latest_choice_wins() {
        let mut input = WeaponInput::default();
        input.cycle(1);
        input.cycle(1);
        assert_eq!(input.take().cycle, 2);
        input.cycle(-1);
        assert!(input.select_key("3"));
        let taken = input.take();
        assert_eq!((taken.cycle, taken.select), (0, Some(WeaponId::Riotgun)));
        assert!(!input.select_key("x"));
    }

    #[test]
    fn clear_releases_fire() {
        let mut input = WeaponInput::default();
        input.set_fire(true);
        input.clear();
        assert!(!input.take().fire);
    }
}
