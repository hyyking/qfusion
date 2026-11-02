//! Placeholder weapon effects drawn on the HUD canvas: tracer lines, projectile dots and impact
//! flashes, projected from world space. Replaced by real effects once models and particles land.

use qf_engine::game::{Projectile, WeaponEvent};

use crate::hud::{Canvas, Rect};
use crate::render::view_projection;

const TRACER_LIFE: f32 = 0.12;
const IMPACT_LIFE: f32 = 0.25;
const MAX_FX: usize = 256;

#[derive(Clone, Copy)]
struct Tracer {
    from: [f32; 3],
    to: [f32; 3],
    age: f32,
}

#[derive(Clone, Copy)]
struct Impact {
    origin: [f32; 3],
    age: f32,
}

#[derive(Default)]
pub struct Fx {
    tracers: Vec<Tracer>,
    impacts: Vec<Impact>,
}

/// Screen position of a world point, or `None` when behind the camera.
fn project(mvp: &[[f32; 4]; 4], point: [f32; 3], size: [f32; 2]) -> Option<[f32; 2]> {
    let clip = |row: usize| {
        mvp[0][row] * point[0] + mvp[1][row] * point[1] + mvp[2][row] * point[2] + mvp[3][row]
    };
    let w = clip(3);
    if w <= 0.01 {
        return None;
    }
    let x = clip(0) / w;
    let y = clip(1) / w;
    Some([(x * 0.5 + 0.5) * size[0], (0.5 - y * 0.5) * size[1]])
}

fn weapon_color(weapon: qf_engine::game::WeaponId) -> [f32; 3] {
    use qf_engine::game::WeaponId::*;
    match weapon {
        Machinegun | Riotgun => [1.0, 0.9, 0.4],
        Lasergun => [1.0, 0.3, 0.3],
        Electrobolt => [0.5, 0.7, 1.0],
        Instagun => [0.6, 1.0, 1.0],
        Plasmagun => [0.4, 1.0, 0.5],
        _ => [1.0, 0.7, 0.3],
    }
}

impl Fx {
    /// Ages existing effects by `dt` seconds and adds those for `events`.
    pub fn update(&mut self, dt: f32, events: &[WeaponEvent]) {
        for t in &mut self.tracers {
            t.age += dt;
        }
        for i in &mut self.impacts {
            i.age += dt;
        }
        self.tracers.retain(|t| t.age < TRACER_LIFE);
        self.impacts.retain(|i| i.age < IMPACT_LIFE);
        for event in events {
            match *event {
                WeaponEvent::Tracer { from, to, .. } if self.tracers.len() < MAX_FX => {
                    self.tracers.push(Tracer { from, to, age: 0.0 });
                }
                WeaponEvent::Impact { origin, .. } if self.impacts.len() < MAX_FX => {
                    self.impacts.push(Impact { origin, age: 0.0 });
                }
                _ => {}
            }
        }
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas,
        projectiles: &[Projectile],
        eye: [f32; 3],
        angles: [f32; 3],
        fov_x: f32,
        size: [f32; 2],
    ) {
        let mvp = view_projection(eye, angles, size[0] / size[1], fov_x);
        for t in &self.tracers {
            let (Some(a), Some(b)) = (project(&mvp, t.from, size), project(&mvp, t.to, size))
            else {
                continue;
            };
            let alpha = 1.0 - t.age / TRACER_LIFE;
            line(canvas, a, b, [1.0, 0.95, 0.6, alpha]);
        }
        for i in &self.impacts {
            if let Some(p) = project(&mvp, i.origin, size) {
                let k = i.age / IMPACT_LIFE;
                let half = 4.0 + 14.0 * k;
                canvas.rects.push(Rect {
                    x: p[0] - half,
                    y: p[1] - half,
                    w: half * 2.0,
                    h: half * 2.0,
                    color: [1.0, 0.6, 0.2, 1.0 - k],
                });
            }
        }
        for p in projectiles {
            if let Some(s) = project(&mvp, p.origin, size) {
                let c = weapon_color(p.weapon);
                canvas.rects.push(Rect {
                    x: s[0] - 3.0,
                    y: s[1] - 3.0,
                    w: 6.0,
                    h: 6.0,
                    color: [c[0], c[1], c[2], 1.0],
                });
            }
        }
    }
}

fn line(canvas: &mut Canvas, a: [f32; 2], b: [f32; 2], color: [f32; 4]) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let steps = (dx.abs().max(dy.abs()) / 2.0).ceil().clamp(1.0, 600.0) as usize;
    for i in 0..=steps {
        let k = i as f32 / steps as f32;
        canvas.rects.push(Rect {
            x: a[0] + dx * k - 1.0,
            y: a[1] + dy * k - 1.0,
            w: 2.0,
            h: 2.0,
            color,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_forward_point_to_screen_center() {
        let mvp = view_projection([0.0; 3], [0.0; 3], 1.0, 90.0);
        let p = project(&mvp, [100.0, 0.0, 0.0], [200.0, 200.0]).unwrap();
        assert!((p[0] - 100.0).abs() < 0.5 && (p[1] - 100.0).abs() < 0.5);
        assert!(project(&mvp, [-100.0, 0.0, 0.0], [200.0, 200.0]).is_none());
    }

    #[test]
    fn effects_expire() {
        let mut fx = Fx::default();
        fx.update(
            0.0,
            &[WeaponEvent::Tracer {
                player: 0,
                weapon: qf_engine::game::WeaponId::Machinegun,
                from: [0.0; 3],
                to: [1.0; 3],
            }],
        );
        assert_eq!(fx.tracers.len(), 1);
        fx.update(1.0, &[]);
        assert!(fx.tracers.is_empty());
    }
}
