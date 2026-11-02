//! Bomb and Defuse loadout choices and their menus.
//!
//! Mirrors the gametype's `primary` / `secondary` command rules (`bomb.rhai`): two distinct
//! primaries out of RL/LG/EB and one secondary out of PG/RG/MG/GL/GB. The script stays
//! authoritative; selections apply on the next round (or immediately in pre-round).

use crate::menu::{Menu, MenuItem, UiAction};

pub const PRIMARIES: [&str; 3] = ["rl", "lg", "eb"];
pub const SECONDARIES: [&str; 5] = ["pg", "rg", "mg", "gl", "gb"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuId {
    Loadout,
    Primary,
    Secondary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Loadout {
    pub primary: [&'static str; 2],
    pub secondary: &'static str,
}

impl Default for Loadout {
    fn default() -> Self {
        Self {
            primary: ["rl", "eb"],
            secondary: "pg",
        }
    }
}

impl Loadout {
    /// `primary` command arguments, or `None` if the pair breaks the gametype rule.
    pub fn primary_args(pair: [&str; 2]) -> Option<String> {
        (pair[0] != pair[1] && pair.iter().all(|p| PRIMARIES.contains(p)))
            .then(|| format!("{} {}", pair[0], pair[1]))
    }

    pub fn is_valid_secondary(weapon: &str) -> bool {
        SECONDARIES.contains(&weapon)
    }

    pub fn primary_pairs() -> Vec<[&'static str; 2]> {
        let mut pairs = Vec::new();
        for (i, a) in PRIMARIES.iter().enumerate() {
            for b in &PRIMARIES[i + 1..] {
                pairs.push([*a, *b]);
            }
        }
        pairs
    }

    pub fn primary_text(&self) -> String {
        format!("{} {}", self.primary[0], self.primary[1]).to_uppercase()
    }

    pub fn menu(&self, id: MenuId) -> Menu {
        match id {
            MenuId::Loadout => Menu::new(
                "LOADOUT",
                vec![
                    MenuItem {
                        label: format!("PRIMARY {}", self.primary_text()),
                        action: UiAction::OpenMenu(MenuId::Primary),
                    },
                    MenuItem {
                        label: format!("SECONDARY {}", self.secondary.to_uppercase()),
                        action: UiAction::OpenMenu(MenuId::Secondary),
                    },
                ],
            ),
            MenuId::Primary => Menu::new(
                "PRIMARY",
                Self::primary_pairs()
                    .into_iter()
                    .map(|pair| MenuItem {
                        label: format!(
                            "{}{} {}",
                            if pair == self.primary { "> " } else { "" },
                            pair[0].to_uppercase(),
                            pair[1].to_uppercase()
                        ),
                        action: UiAction::SetPrimary(pair),
                    })
                    .collect(),
            ),
            MenuId::Secondary => Menu::new(
                "SECONDARY",
                SECONDARIES
                    .iter()
                    .map(|weapon| MenuItem {
                        label: format!(
                            "{}{}",
                            if *weapon == self.secondary { "> " } else { "" },
                            weapon.to_uppercase()
                        ),
                        action: UiAction::SetSecondary(weapon),
                    })
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_rules_match_the_script() {
        assert_eq!(
            Loadout::primary_args(["rl", "eb"]).as_deref(),
            Some("rl eb")
        );
        assert_eq!(Loadout::primary_args(["rl", "rl"]), None);
        assert_eq!(Loadout::primary_args(["rl", "pg"]), None);
        assert!(Loadout::is_valid_secondary("gb"));
        assert!(!Loadout::is_valid_secondary("rl"));
    }

    #[test]
    fn pairs_are_all_valid_and_distinct() {
        let pairs = Loadout::primary_pairs();
        assert_eq!(pairs.len(), 3);
        for pair in pairs {
            assert!(Loadout::primary_args(pair).is_some());
        }
    }

    #[test]
    fn menus_mark_current_selection() {
        let loadout = Loadout::default();
        let primary = loadout.menu(MenuId::Primary);
        let labels: Vec<_> = primary.items().iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["RL LG", "> RL EB", "LG EB"]);
        let secondary = loadout.menu(MenuId::Secondary);
        assert_eq!(secondary.items()[0].label, "> PG");
        assert_eq!(secondary.items().len(), SECONDARIES.len());
        let root = loadout.menu(MenuId::Loadout);
        assert_eq!(root.items()[0].label, "PRIMARY RL EB");
    }
}
