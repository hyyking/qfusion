//! Per-element HUD layout (enabled, anchor, scale, colour) persisted as TOML.
//!
//! Client-only presentation data: it never enters the sim or demo state.
// Consumed incrementally by the menu and customization screens.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::hud::{Anchor, HAnchor, VAnchor};

pub const UPS: &str = "ups";
pub const FPS: &str = "fps";
pub const RECORD: &str = "record";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ElementLayout {
    pub enabled: bool,
    pub anchor: Anchor,
    /// Multiplies the element's font pixel size.
    pub scale: f32,
    /// Overrides the element's main colour (RGBA, 0..1) when set.
    pub color: Option<[f32; 4]>,
}

impl ElementLayout {
    pub const MIN_SCALE: f32 = 0.5;
    pub const MAX_SCALE: f32 = 4.0;

    pub const fn new(horizontal: HAnchor, vertical: VAnchor, margin: [f32; 2]) -> Self {
        Self {
            enabled: true,
            anchor: Anchor {
                horizontal,
                vertical,
                margin,
            },
            scale: 1.0,
            color: None,
        }
    }

    /// Replaces out-of-range or non-finite values from a hand-edited file.
    fn sanitized(mut self, default: &ElementLayout) -> Self {
        if !self.scale.is_finite() {
            self.scale = default.scale;
        }
        self.scale = self.scale.clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        if self.anchor.margin.iter().any(|m| !m.is_finite()) {
            self.anchor.margin = default.anchor.margin;
        }
        if let Some(c) = self.color
            && c.iter().any(|v| !v.is_finite())
        {
            self.color = None;
        }
        self.color = self.color.map(|c| c.map(|v| v.clamp(0.0, 1.0)));
        self
    }
}

impl Default for ElementLayout {
    fn default() -> Self {
        Self::new(HAnchor::Left, VAnchor::Top, [16.0, 16.0])
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HudLayout {
    pub elements: BTreeMap<String, ElementLayout>,
}

impl HudLayout {
    pub fn default_for(name: &str) -> ElementLayout {
        match name {
            UPS => ElementLayout::new(HAnchor::Center, VAnchor::Bottom, [0.0, 48.0]),
            FPS => ElementLayout::new(HAnchor::Right, VAnchor::Top, [16.0, 16.0]),
            RECORD => ElementLayout::new(HAnchor::Left, VAnchor::Top, [16.0, 16.0]),
            _ => ElementLayout::default(),
        }
    }

    /// Layout of `name`, falling back to its built-in default.
    pub fn get(&self, name: &str) -> ElementLayout {
        self.elements
            .get(name)
            .copied()
            .unwrap_or_else(|| Self::default_for(name))
    }

    pub fn set(&mut self, name: &str, layout: ElementLayout) {
        self.elements.insert(name.to_owned(), layout);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        let mut layout: Self = toml::from_str(text)?;
        for (name, element) in &mut layout.elements {
            *element = element.sanitized(&Self::default_for(name));
        }
        Ok(layout)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("layout serializes")
    }

    /// Loads `path`; a missing or invalid file yields defaults (the error is returned for logging).
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match Self::parse(&text) {
                Ok(layout) => (layout, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_toml())
    }
}

impl Default for HudLayout {
    fn default() -> Self {
        Self {
            elements: [UPS, FPS, RECORD]
                .into_iter()
                .map(|name| (name.to_owned(), Self::default_for(name)))
                .collect(),
        }
    }
}

/// User config directory for qfusion (`%APPDATA%\qfusion`, `$XDG_CONFIG_HOME/qfusion`
/// or `~/.config/qfusion`).
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("qfusion"))
}

pub fn layout_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("hud.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_original_placement() {
        let layout = HudLayout::default();
        let ups = layout.get(UPS);
        assert_eq!(ups.anchor.horizontal, HAnchor::Center);
        assert_eq!(ups.anchor.vertical, VAnchor::Bottom);
        assert_eq!(ups.anchor.margin, [0.0, 48.0]);
        assert!(ups.enabled);
        assert_eq!(layout.get("unknown"), ElementLayout::default());
    }

    #[test]
    fn round_trips_through_toml() {
        let mut layout = HudLayout::default();
        let mut fps = layout.get(FPS);
        fps.enabled = false;
        fps.scale = 2.0;
        fps.color = Some([1.0, 0.5, 0.0, 1.0]);
        fps.anchor = Anchor {
            horizontal: HAnchor::Left,
            vertical: VAnchor::Middle,
            margin: [4.0, 8.0],
        };
        layout.set(FPS, fps);
        assert_eq!(HudLayout::parse(&layout.to_toml()).unwrap(), layout);
    }

    #[test]
    fn partial_file_falls_back_per_field() {
        let layout = HudLayout::parse("[elements.fps]\nenabled = false\n").unwrap();
        let fps = layout.get(FPS);
        assert!(!fps.enabled);
        assert_eq!(fps.scale, 1.0);
        assert_eq!(layout.get(UPS), HudLayout::default_for(UPS));
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let layout = HudLayout::parse(
            "[elements.fps]\nscale = 100.0\ncolor = [2.0, -1.0, 0.5, 1.0]\n",
        )
        .unwrap();
        let fps = layout.get(FPS);
        assert_eq!(fps.scale, ElementLayout::MAX_SCALE);
        assert_eq!(fps.color, Some([1.0, 0.0, 0.5, 1.0]));
    }

    #[test]
    fn invalid_or_missing_file_yields_defaults() {
        let dir = std::env::temp_dir().join(format!("qf-hud-layout-{}", std::process::id()));
        let path = dir.join("hud.toml");
        let (layout, error) = HudLayout::load(&path);
        assert_eq!(layout, HudLayout::default());
        assert!(error.is_none());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "elements = 3 ???").unwrap();
        let (layout, error) = HudLayout::load(&path);
        assert_eq!(layout, HudLayout::default());
        assert!(error.is_some());

        let mut custom = HudLayout::default();
        custom.reset();
        custom.save(&path).unwrap();
        assert_eq!(HudLayout::load(&path), (custom, None));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
