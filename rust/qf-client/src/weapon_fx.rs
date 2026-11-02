//! Client-side weapon presentation: weapon HUD, tracer/projectile/impact markers and
//! sound cues. Everything here is pure (no GPU, window or audio device types); it only
//! reads engine state and events and never affects the simulation.

use qf_engine::game::{Inventory, Projectile, WeaponEvent, WeaponId, WeaponState};

use crate::hud::{Anchor, Canvas, HAnchor, Rect, Screen, VAnchor, text_size};
use crate::math::Mat4;

const TRACER_TICKS: u32 = 20;
const IMPACT_TICKS: u32 = 25;
const LINE_SAMPLES: usize = 32;

/// Number-row key for a weapon: `1` is the gunblade ... `0` is the instagun.
pub fn weapon_for_key(key: &str) -> Option<WeaponId> {
    let digit = key
        .chars()
        .next()
        .filter(|_| key.len() == 1)?
        .to_digit(10)?;
    let index = (digit as usize + 9) % 10;
    WeaponId::ALL.get(index).copied()
}

fn slot_label(weapon: WeaponId) -> u32 {
    (weapon.index() as u32 + 1) % 10
}

fn weapon_color(weapon: WeaponId) -> [f32; 4] {
    match weapon {
        WeaponId::Gunblade => [1.0, 0.9, 0.4, 1.0],
        WeaponId::Machinegun | WeaponId::Riotgun => [1.0, 0.85, 0.3, 1.0],
        WeaponId::GrenadeLauncher => [0.5, 1.0, 0.4, 1.0],
        WeaponId::RocketLauncher => [1.0, 0.5, 0.2, 1.0],
        WeaponId::Plasmagun => [0.4, 1.0, 0.7, 1.0],
        WeaponId::Lasergun => [1.0, 0.8, 0.2, 1.0],
        WeaponId::Electrobolt | WeaponId::Shockwave => [0.4, 0.7, 1.0, 1.0],
        WeaponId::Instagun => [0.9, 0.5, 1.0, 1.0],
    }
}

/// Screen position of a world point, `None` when it is behind the camera.
pub fn project(mvp: &Mat4, point: [f32; 3], screen: &Screen) -> Option<[f32; 2]> {
    let row =
        |r: usize| mvp[0][r] * point[0] + mvp[1][r] * point[1] + mvp[2][r] * point[2] + mvp[3][r];
    let w = row(3);
    if w <= 0.1 {
        return None;
    }
    let (x, y) = (row(0) / w, row(1) / w);
    Some([
        (x * 0.5 + 0.5) * screen.width,
        (0.5 - y * 0.5) * screen.height,
    ])
}

/// A sound to play for a weapon event; `origin` `None` means local (own weapon).
#[derive(Clone, Debug, PartialEq)]
pub struct SoundCue {
    pub path: &'static str,
    pub origin: Option<[f32; 3]>,
}

/// Sounds that exist in the legacy resource list (`gs_qrespath.h`). Weapons whose
/// firing sounds live only in the legacy model configs stay silent.
fn fire_sound(weapon: WeaponId) -> Option<&'static str> {
    match weapon {
        WeaponId::Gunblade => Some("sounds/weapons/bladegun_strong_fire"),
        _ => None,
    }
}

fn impact_sound(weapon: WeaponId) -> Option<&'static str> {
    Some(match weapon {
        WeaponId::Gunblade => "sounds/weapons/bladegun_strong_hit_1",
        WeaponId::Riotgun => "sounds/weapons/riotgun_strong_hit",
        WeaponId::GrenadeLauncher => "sounds/weapons/gren_strong_explosion",
        WeaponId::RocketLauncher => "sounds/weapons/rocket_strong_explosion",
        WeaponId::Plasmagun => "sounds/weapons/plasmagun_strong_explosion",
        WeaponId::Lasergun => "sounds/weapons/laser_hit0",
        WeaponId::Electrobolt => "sounds/weapons/electrobolt_hit",
        WeaponId::Shockwave => "sounds/weapons/shockwave_strong_explosion",
        WeaponId::Machinegun | WeaponId::Instagun => return None,
    })
}

/// Every sound path the weapon cues can request, for preloading.
pub fn sound_paths() -> Vec<&'static str> {
    WeaponId::ALL
        .into_iter()
        .flat_map(|weapon| [fire_sound(weapon), impact_sound(weapon)])
        .flatten()
        .collect()
}

struct Tracer {
    from: [f32; 3],
    to: [f32; 3],
    weapon: WeaponId,
    age: u32,
}

struct Impact {
    origin: [f32; 3],
    weapon: WeaponId,
    age: u32,
}

#[derive(Default)]
pub struct Effects {
    tracers: Vec<Tracer>,
    impacts: Vec<Impact>,
    projectiles: Vec<([f32; 3], WeaponId)>,
}

impl Effects {
    /// Records an engine event and returns the sound it should make, if any.
    pub fn push_event(&mut self, event: &WeaponEvent, local_player: usize) -> Option<SoundCue> {
        match *event {
            WeaponEvent::Fired {
                player,
                weapon,
                origin,
                ..
            } => fire_sound(weapon).map(|path| SoundCue {
                path,
                origin: (player != local_player).then_some(origin),
            }),
            WeaponEvent::Tracer {
                weapon, from, to, ..
            } => {
                self.tracers.push(Tracer {
                    from,
                    to,
                    weapon,
                    age: 0,
                });
                None
            }
            WeaponEvent::Impact { weapon, origin, .. } => {
                self.impacts.push(Impact {
                    origin,
                    weapon,
                    age: 0,
                });
                impact_sound(weapon).map(|path| SoundCue {
                    path,
                    origin: Some(origin),
                })
            }
            WeaponEvent::Projectile { .. } | WeaponEvent::Damage { .. } => None,
        }
    }

    pub fn set_projectiles(&mut self, projectiles: &[Projectile]) {
        self.projectiles = projectiles.iter().map(|p| (p.origin, p.weapon)).collect();
    }

    pub fn advance(&mut self, ticks: u32) {
        for tracer in &mut self.tracers {
            tracer.age += ticks;
        }
        for impact in &mut self.impacts {
            impact.age += ticks;
        }
        self.tracers.retain(|t| t.age < TRACER_TICKS);
        self.impacts.retain(|i| i.age < IMPACT_TICKS);
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.tracers.is_empty() && self.impacts.is_empty() && self.projectiles.is_empty()
    }

    pub fn draw(&self, mvp: &Mat4, screen: &Screen, canvas: &mut Canvas) {
        let size = 2.0 * screen.scale;
        for tracer in &self.tracers {
            let (Some(a), Some(b)) = (
                project(mvp, tracer.from, screen),
                project(mvp, tracer.to, screen),
            ) else {
                continue;
            };
            let mut color = weapon_color(tracer.weapon);
            color[3] = 1.0 - tracer.age as f32 / TRACER_TICKS as f32;
            for i in 0..=LINE_SAMPLES {
                let t = i as f32 / LINE_SAMPLES as f32;
                canvas.rects.push(Rect {
                    x: a[0] + (b[0] - a[0]) * t - size * 0.5,
                    y: a[1] + (b[1] - a[1]) * t - size * 0.5,
                    w: size,
                    h: size,
                    color,
                });
            }
        }
        for &(origin, weapon) in &self.projectiles {
            if let Some(at) = project(mvp, origin, screen) {
                let side = 8.0 * screen.scale;
                canvas.rects.push(Rect {
                    x: at[0] - side * 0.5,
                    y: at[1] - side * 0.5,
                    w: side,
                    h: side,
                    color: weapon_color(weapon),
                });
            }
        }
        for impact in &self.impacts {
            if let Some(at) = project(mvp, impact.origin, screen) {
                let progress = impact.age as f32 / IMPACT_TICKS as f32;
                let side = (6.0 + 26.0 * progress) * screen.scale;
                let mut color = weapon_color(impact.weapon);
                color[3] = 1.0 - progress;
                canvas.rects.push(Rect {
                    x: at[0] - side * 0.5,
                    y: at[1] - side * 0.5,
                    w: side,
                    h: side,
                    color,
                });
            }
        }
    }
}

/// Owned weapons and the held one, for the HUD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WeaponHud {
    pub slots: Vec<(WeaponId, u32)>,
    pub held: Option<WeaponId>,
    pub switching: bool,
}

impl WeaponHud {
    pub fn from_inventory(inventory: &Inventory) -> Self {
        Self {
            slots: inventory.owned().collect(),
            held: inventory.target(),
            switching: matches!(
                inventory.state(),
                WeaponState::SwitchingUp | WeaponState::SwitchingDown
            ),
        }
    }

    pub fn draw(&self, screen: &Screen, canvas: &mut Canvas) {
        let pixel = 2.0 * screen.scale;
        let row_height = 10.0 * pixel;
        for (row, &(weapon, ammo)) in self.slots.iter().rev().enumerate() {
            let def = weapon.def();
            let ammo_text = if def.ammo_max == 0 {
                "-".to_owned()
            } else {
                ammo.to_string()
            };
            let text = format!(
                "{} {} {}",
                slot_label(weapon),
                def.short_name.to_ascii_uppercase(),
                ammo_text
            );
            let held = self.held == Some(weapon);
            let color = if held {
                [1.0, 0.9, 0.3, 1.0]
            } else if def.ammo_max > 0 && ammo == 0 {
                [0.6, 0.3, 0.3, 0.8]
            } else {
                [0.85, 0.85, 0.85, 0.9]
            };
            let anchor = Anchor {
                horizontal: HAnchor::Right,
                vertical: VAnchor::Bottom,
                margin: [16.0, 16.0],
            };
            let size = text_size(text.chars().count(), pixel);
            let [x, y] = anchor.place(screen, size);
            canvas.shadowed_text(&text, [x, y - row as f32 * row_height], pixel, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::view_projection;

    const SCREEN: Screen = Screen {
        width: 1280.0,
        height: 720.0,
        scale: 1.0,
    };

    #[test]
    fn number_keys_map_to_weapons() {
        assert_eq!(weapon_for_key("1"), Some(WeaponId::Gunblade));
        assert_eq!(weapon_for_key("5"), Some(WeaponId::RocketLauncher));
        assert_eq!(weapon_for_key("0"), Some(WeaponId::Instagun));
        assert_eq!(weapon_for_key("a"), None);
        assert_eq!(weapon_for_key("12"), None);
    }

    #[test]
    fn points_ahead_project_to_the_center_and_behind_are_culled() {
        let mvp = view_projection([0.0; 3], [0.0, 0.0, 0.0], 16.0 / 9.0);
        let at = project(&mvp, [100.0, 0.0, 0.0], &SCREEN).unwrap();
        assert!(
            (at[0] - 640.0).abs() < 1.0 && (at[1] - 360.0).abs() < 1.0,
            "{at:?}"
        );
        assert!(project(&mvp, [-100.0, 0.0, 0.0], &SCREEN).is_none());
        let above = project(&mvp, [100.0, 0.0, 50.0], &SCREEN).unwrap();
        assert!(above[1] < 360.0, "up is up on screen");
    }

    #[test]
    fn effects_expire_and_make_sounds() {
        let mut effects = Effects::default();
        let cue = effects.push_event(
            &WeaponEvent::Fired {
                player: 0,
                weapon: WeaponId::Gunblade,
                origin: [0.0; 3],
                direction: [1.0, 0.0, 0.0],
            },
            0,
        );
        assert_eq!(cue.unwrap().origin, None);
        let cue = effects.push_event(
            &WeaponEvent::Impact {
                weapon: WeaponId::RocketLauncher,
                origin: [5.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                victim: None,
            },
            0,
        );
        assert_eq!(cue.unwrap().origin, Some([5.0, 0.0, 0.0]));
        effects.push_event(
            &WeaponEvent::Tracer {
                player: 0,
                weapon: WeaponId::Electrobolt,
                from: [0.0; 3],
                to: [100.0, 0.0, 0.0],
            },
            0,
        );
        assert!(!effects.is_empty());
        effects.advance(TRACER_TICKS.max(IMPACT_TICKS));
        assert!(effects.is_empty());
    }

    #[test]
    fn hud_lists_owned_weapons_inside_the_screen() {
        let mut inventory = Inventory::default();
        inventory.give_weapon(WeaponId::Gunblade, 0);
        inventory.give_weapon(WeaponId::RocketLauncher, 5);
        let hud = WeaponHud::from_inventory(&inventory);
        assert_eq!(hud.slots.len(), 2);
        assert_eq!(hud.held, Some(WeaponId::Gunblade));
        let mut canvas = Canvas::default();
        hud.draw(&SCREEN, &mut canvas);
        assert!(!canvas.rects.is_empty());
        for r in &canvas.rects {
            assert!(r.x >= 0.0 && r.y >= 0.0);
            assert!(r.x + r.w <= SCREEN.width && r.y + r.h <= SCREEN.height);
        }
    }

    #[test]
    fn tracers_and_projectiles_draw() {
        let mut effects = Effects::default();
        effects.push_event(
            &WeaponEvent::Tracer {
                player: 0,
                weapon: WeaponId::Machinegun,
                from: [50.0, -10.0, -5.0],
                to: [500.0, 40.0, 10.0],
            },
            0,
        );
        effects.set_projectiles(&[Projectile {
            id: 0,
            owner: 0,
            weapon: WeaponId::RocketLauncher,
            origin: [200.0, 0.0, 0.0],
            velocity: [0.0; 3],
            age_ms: 0,
        }]);
        let mvp = view_projection([0.0; 3], [0.0; 3], 16.0 / 9.0);
        let mut canvas = Canvas::default();
        effects.draw(&mvp, &SCREEN, &mut canvas);
        assert!(canvas.rects.len() > LINE_SAMPLES);
    }
}
