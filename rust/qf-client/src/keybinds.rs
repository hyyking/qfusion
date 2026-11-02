//! Configurable character-key bindings for client actions.
//!
//! Stored as TOML next to `hud.toml` (`keybinds.toml`):
//!
//! ```toml
//! [keys]
//! team_menu = "m"
//! loadout_menu = "l"
//! ```
//!
//! Unknown actions are ignored and invalid values fall back to the default
//! for that action. Duplicate keys are rejected (the later action in
//! [`Action::ALL`] order reverts to its default) so one key never fires two actions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Action {
    SoundTest,
    DropBomb,
    Suicide,
    LoadoutMenu,
    TeamMenu,
    HudEditor,
}

impl Action {
    pub const ALL: [Action; 6] = [
        Action::SoundTest,
        Action::DropBomb,
        Action::Suicide,
        Action::LoadoutMenu,
        Action::TeamMenu,
        Action::HudEditor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Action::SoundTest => "sound_test",
            Action::DropBomb => "drop_bomb",
            Action::Suicide => "suicide",
            Action::LoadoutMenu => "loadout_menu",
            Action::TeamMenu => "team_menu",
            Action::HudEditor => "hud_editor",
        }
    }

    pub fn default_key(self) -> char {
        match self {
            Action::SoundTest => 'b',
            Action::DropBomb => 'g',
            Action::Suicide => 'k',
            Action::LoadoutMenu => 'l',
            Action::TeamMenu => 'm',
            Action::HudEditor => 'h',
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybinds {
    keys: BTreeMap<Action, char>,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            keys: Action::ALL.iter().map(|a| (*a, a.default_key())).collect(),
        }
    }
}

impl Keybinds {
    pub fn key(&self, action: Action) -> char {
        self.keys
            .get(&action)
            .copied()
            .unwrap_or_else(|| action.default_key())
    }

    /// The action bound to a typed character (case-insensitive).
    pub fn action_for(&self, typed: &str) -> Option<Action> {
        let mut chars = typed.chars();
        let c = chars.next()?.to_ascii_lowercase();
        if chars.next().is_some() {
            return None;
        }
        Action::ALL.into_iter().find(|a| self.key(*a) == c)
    }

    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let file: File = match toml::from_str(text) {
            Ok(file) => file,
            Err(error) => {
                return (Self::default(), vec![format!("invalid keybinds: {error}")]);
            }
        };
        let mut binds = Self::default();
        for (name, value) in &file.keys {
            let Some(action) = Action::ALL.into_iter().find(|a| a.name() == name) else {
                warnings.push(format!("unknown action `{name}`"));
                continue;
            };
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_ascii_graphic() && c != ' ' => {
                    binds.keys.insert(action, c.to_ascii_lowercase());
                }
                _ => warnings.push(format!("`{name}`: expected a single printable key")),
            }
        }
        // Resolve duplicates by reverting the later action to its default.
        for (i, action) in Action::ALL.iter().enumerate() {
            let key = binds.key(*action);
            if Action::ALL[..i].iter().any(|a| binds.key(*a) == key) {
                warnings.push(format!("`{}`: key `{key}` already bound", action.name()));
                binds.keys.insert(*action, action.default_key());
            }
        }
        (binds, warnings)
    }

    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Self::default(), Vec::new())
            }
            Err(error) => (
                Self::default(),
                vec![format!("{}: {error}", path.display())],
            ),
        }
    }
}

pub fn keybinds_path() -> Option<PathBuf> {
    crate::hud_layout::config_dir().map(|dir| dir.join("keybinds.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_map_back_to_actions() {
        let binds = Keybinds::default();
        for action in Action::ALL {
            assert_eq!(
                binds.action_for(&action.default_key().to_string()),
                Some(action)
            );
        }
        assert_eq!(binds.action_for("M"), Some(Action::TeamMenu));
        assert_eq!(binds.action_for("mm"), None);
        assert_eq!(binds.action_for("z"), None);
    }

    #[test]
    fn overrides_apply_and_unknown_is_warned() {
        let (binds, warnings) = Keybinds::parse("[keys]\nteam_menu = \"T\"\nbogus = \"x\"\n");
        assert_eq!(binds.action_for("t"), Some(Action::TeamMenu));
        assert_eq!(binds.action_for("m"), None);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn duplicates_and_invalid_values_fall_back() {
        let (binds, warnings) =
            Keybinds::parse("[keys]\nteam_menu = \"l\"\nloadout_menu = \"\"\nsuicide = \"ab\"\n");
        // team_menu = l collides with loadout's default; the later action (team) reverts.
        assert_eq!(binds.key(Action::LoadoutMenu), 'l');
        assert_eq!(binds.key(Action::TeamMenu), 'm');
        assert_eq!(binds.key(Action::Suicide), 'k');
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn broken_toml_gives_defaults() {
        let (binds, warnings) = Keybinds::parse("[keys");
        assert_eq!(binds, Keybinds::default());
        assert_eq!(warnings.len(), 1);
    }
}
