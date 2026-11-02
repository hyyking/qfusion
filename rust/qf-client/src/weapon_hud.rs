//! Weapon bar, current-weapon label and ammo counter.
//!
//! Pure drawing from a [`WeaponSnapshot`] taken from the local player's [`Inventory`]. The
//! client still runs on `ModeSimulation` and has no inventory, so the snapshot stays `None`
//! (nothing is drawn) until the client moves onto `Sim`; then `WeaponSnapshot::from_inventory`
//! is fed once per frame, outside the sim.
// Fed by the client once it runs on `Sim`.
#![allow(dead_code)]

use std::fmt::Write;

use qf_engine::game::{Inventory, WeaponId};

use crate::hud::{Canvas, Rect, Screen, text_size};
use crate::hud_layout::{self, ElementLayout, HudLayout};

pub const LOW_AMMO: u32 = 5;

#[derive(Clone, Debug, PartialEq)]
pub struct WeaponSnapshot {
    /// Weapon being used (or switching to, if a switch is pending).
    pub selected: Option<WeaponId>,
    /// Owned weapons in canonical order with their ammo.
    pub slots: Vec<(WeaponId, u32)>,
}

impl WeaponSnapshot {
    pub fn from_inventory(inventory: &Inventory) -> Self {
        Self {
            selected: inventory.pending().or(inventory.selected()),
            slots: inventory.owned().collect(),
        }
    }

    pub fn selected_ammo(&self) -> Option<u32> {
        let selected = self.selected?;
        self.slots.iter().find(|(w, _)| *w == selected).map(|s| s.1)
    }
}

pub struct WeaponHud {
    pub snapshot: Option<WeaponSnapshot>,
    bar: ElementLayout,
    ammo: ElementLayout,
}

impl WeaponHud {
    const PIXEL: f32 = 3.0;

    pub fn new() -> Self {
        Self {
            snapshot: None,
            bar: HudLayout::default_for(hud_layout::WEAPONS),
            ammo: HudLayout::default_for(hud_layout::AMMO),
        }
    }

    /// Approximate on-screen size of the ammo block (name + counter), in pixels.
    pub fn ammo_size(layout_scale: f32, screen_scale: f32) -> [f32; 2] {
        let pixel = Self::PIXEL * layout_scale * screen_scale;
        let name = text_size(12, pixel * 0.5);
        let ammo = text_size(3, pixel * 2.0);
        [name[0].max(ammo[0]), name[1] + pixel + ammo[1]]
    }

    /// On-screen size of a bar with slots weapons, in pixels.
    pub fn bar_size(slots: usize, layout_scale: f32, screen_scale: f32) -> [f32; 2] {
        let pixel = Self::PIXEL * layout_scale * screen_scale;
        let cell = Self::cell_size(pixel);
        let count = slots as f32;
        [count * cell[0] + (count - 1.0).max(0.0) * pixel, cell[1]]
    }

    fn cell_size(pixel: f32) -> [f32; 2] {
        let text = text_size(4, pixel * 0.75);
        [text[0] + pixel * 2.0, text[1] * 2.0 + pixel * 3.0]
    }

    pub fn set_layout(&mut self, layout: &HudLayout) {
        self.bar = layout.get(hud_layout::WEAPONS);
        self.ammo = layout.get(hud_layout::AMMO);
    }

    pub fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        self.draw_ammo(snapshot, screen, canvas);
        self.draw_bar(snapshot, screen, canvas);
    }

    fn draw_ammo(&self, snapshot: &WeaponSnapshot, screen: &Screen, canvas: &mut Canvas) {
        let layout = &self.ammo;
        let Some(weapon) = snapshot.selected else {
            return;
        };
        if !layout.enabled {
            return;
        }
        let pixel = Self::PIXEL * layout.scale * screen.scale;
        let name = weapon.def().name.to_uppercase();
        let mut ammo = String::new();
        let _ = write!(ammo, "{}", snapshot.selected_ammo().unwrap_or(0));
        let name_size = text_size(name.chars().count(), pixel * 0.5);
        let ammo_size = text_size(ammo.chars().count(), pixel * 2.0);
        let gap = pixel;
        let size = [
            name_size[0].max(ammo_size[0]),
            name_size[1] + gap + ammo_size[1],
        ];
        let [x, y] = layout.anchor.place(screen, size);
        canvas.shadowed_text(
            &name,
            [x + (size[0] - name_size[0]) * 0.5, y],
            pixel * 0.5,
            [0.8, 0.8, 0.8, 1.0],
        );
        let low = snapshot.selected_ammo().unwrap_or(0) <= LOW_AMMO;
        let default = if low {
            [1.0, 0.25, 0.2, 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };
        canvas.shadowed_text(
            &ammo,
            [x + (size[0] - ammo_size[0]) * 0.5, y + name_size[1] + gap],
            pixel * 2.0,
            if low {
                default
            } else {
                layout.color.unwrap_or(default)
            },
        );
    }

    fn draw_bar(&self, snapshot: &WeaponSnapshot, screen: &Screen, canvas: &mut Canvas) {
        let layout = &self.bar;
        if !layout.enabled || snapshot.slots.is_empty() {
            return;
        }
        let pixel = Self::PIXEL * layout.scale * screen.scale;
        let cell_chars = 4;
        let text = text_size(cell_chars, pixel * 0.75);
        let pad = pixel;
        let cell = [text[0] + pad * 2.0, text[1] * 2.0 + pad * 3.0];
        let gap = pad;
        let count = snapshot.slots.len() as f32;
        let size = [count * cell[0] + (count - 1.0) * gap, cell[1]];
        let [x, y] = layout.anchor.place(screen, size);
        let color = layout.color.unwrap_or([1.0, 1.0, 1.0, 1.0]);
        for (i, (weapon, ammo)) in snapshot.slots.iter().enumerate() {
            let cx = x + i as f32 * (cell[0] + gap);
            let selected = snapshot.selected == Some(*weapon);
            canvas.rects.push(Rect {
                x: cx,
                y,
                w: cell[0],
                h: cell[1],
                color: if selected {
                    [0.25, 0.3, 0.45, 0.85]
                } else {
                    [0.0, 0.0, 0.0, 0.45]
                },
            });
            let tint = if selected {
                color
            } else {
                [color[0] * 0.7, color[1] * 0.7, color[2] * 0.7, color[3]]
            };
            canvas.text(
                &weapon.def().short_name.to_uppercase(),
                [cx + pad, y + pad],
                pixel * 0.75,
                tint,
            );
            let low = *ammo <= LOW_AMMO;
            canvas.text(
                &ammo.to_string(),
                [cx + pad, y + pad * 2.0 + text[1]],
                pixel * 0.75,
                if low { [1.0, 0.25, 0.2, 1.0] } else { tint },
            );
        }
    }
}

impl Default for WeaponHud {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Screen = Screen {
        width: 1000.0,
        height: 600.0,
        scale: 1.0,
    };

    fn inventory() -> Inventory {
        let mut inv = Inventory::default();
        inv.give_weapon(WeaponId::RocketLauncher, 10);
        inv.give_weapon(WeaponId::Electrobolt, 3);
        inv
    }

    #[test]
    fn snapshot_lists_owned_weapons_and_selected_ammo() {
        let mut inv = inventory();
        inv.select_best();
        let snap = WeaponSnapshot::from_inventory(&inv);
        assert!(snap.slots.contains(&(WeaponId::RocketLauncher, 10)));
        assert!(snap.slots.iter().all(|(w, _)| inv.owns(*w)));
        let selected = snap.selected.expect("a weapon is selected");
        assert_eq!(snap.selected_ammo(), Some(inv.ammo(selected)));
    }

    #[test]
    fn nothing_is_drawn_without_a_snapshot() {
        let hud = WeaponHud::new();
        let mut canvas = Canvas::default();
        hud.draw(&SCREEN, &mut canvas);
        assert!(canvas.rects.is_empty());
    }

    #[test]
    fn draws_inside_the_screen_and_respects_disable() {
        let mut inv = inventory();
        inv.select_best();
        let mut hud = WeaponHud::new();
        hud.snapshot = Some(WeaponSnapshot::from_inventory(&inv));
        let mut canvas = Canvas::default();
        hud.draw(&SCREEN, &mut canvas);
        assert!(!canvas.rects.is_empty());
        for r in &canvas.rects {
            assert!(r.x >= 0.0 && r.y >= 0.0, "{r:?}");
            assert!(r.x + r.w <= SCREEN.width + 0.01 && r.y + r.h <= SCREEN.height + 0.01);
        }

        let mut layout = HudLayout::default();
        for name in [hud_layout::WEAPONS, hud_layout::AMMO] {
            let mut element = layout.get(name);
            element.enabled = false;
            layout.set(name, element);
        }
        hud.set_layout(&layout);
        canvas.clear();
        hud.draw(&SCREEN, &mut canvas);
        assert!(canvas.rects.is_empty());
    }

    #[test]
    fn low_ammo_turns_red() {
        let mut hud = WeaponHud::new();
        hud.snapshot = Some(WeaponSnapshot {
            selected: Some(WeaponId::Electrobolt),
            slots: vec![(WeaponId::Electrobolt, 1)],
        });
        let mut canvas = Canvas::default();
        hud.draw(&SCREEN, &mut canvas);
        assert!(
            canvas
                .rects
                .iter()
                .any(|r| r.color == [1.0, 0.25, 0.2, 1.0])
        );
    }
}
