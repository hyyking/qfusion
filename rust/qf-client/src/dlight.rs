//! Dynamic effect lights (explosions, rocket glow). Render-only: nothing here feeds back into
//! the simulation. The renderers receive the brightest few as a fixed uniform array.

pub const MAX_LIGHTS: usize = 4;
/// Packed layout shared with the shaders: per light `[x, y, z, radius]` then `[r, g, b, 0]`.
pub type PackedLights = [[f32; 4]; MAX_LIGHTS * 2];

/// Warm colour of the legacy rocket and generic explosion light.
const EXPLOSION_COLOR: [f32; 3] = [1.0, 0.75, 0.4];
const EXPLOSION_SECONDS: f32 = 0.6;
const EXPLOSION_MIN_RADIUS: f32 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicLight {
    pub origin: [f32; 3],
    pub radius: f32,
    pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct Flash {
    origin: [f32; 3],
    radius: f32,
    color: [f32; 3],
    age: f32,
    life: f32,
}

/// Short-lived lights that fade out linearly.
#[derive(Debug, Default)]
pub struct EffectLights {
    flashes: Vec<Flash>,
}

impl EffectLights {
    pub fn explosion(&mut self, origin: [f32; 3], radius: f32) {
        self.flashes.push(Flash {
            origin,
            radius: radius.max(EXPLOSION_MIN_RADIUS),
            color: EXPLOSION_COLOR,
            age: 0.0,
            life: EXPLOSION_SECONDS,
        });
    }

    pub fn advance(&mut self, dt: f32) {
        for flash in &mut self.flashes {
            flash.age += dt;
        }
        self.flashes.retain(|f| f.age < f.life);
    }

    /// Current lights with their colour faded by remaining life.
    pub fn lights(&self) -> Vec<DynamicLight> {
        self.flashes
            .iter()
            .map(|f| {
                let fade = 1.0 - f.age / f.life;
                DynamicLight {
                    origin: f.origin,
                    radius: f.radius,
                    color: f.color.map(|c| c * fade),
                }
            })
            .collect()
    }
}

/// Packs up to [`MAX_LIGHTS`] lights (the brightest win) and returns the count used.
pub fn pack(lights: &[DynamicLight]) -> (PackedLights, u32) {
    let mut sorted: Vec<&DynamicLight> = lights.iter().collect();
    let power = |l: &DynamicLight| l.color.iter().sum::<f32>() * l.radius;
    sorted.sort_by(|a, b| power(b).total_cmp(&power(a)));
    let mut packed = [[0.0; 4]; MAX_LIGHTS * 2];
    let count = sorted.len().min(MAX_LIGHTS);
    for (i, l) in sorted.iter().take(MAX_LIGHTS).enumerate() {
        packed[i * 2] = [l.origin[0], l.origin[1], l.origin[2], l.radius];
        packed[i * 2 + 1] = [l.color[0], l.color[1], l.color[2], 0.0];
    }
    (packed, count as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_fades_and_expires() {
        let mut fx = EffectLights::default();
        fx.explosion([1.0, 2.0, 3.0], 400.0);
        assert_eq!(fx.lights()[0].radius, 400.0);
        fx.advance(EXPLOSION_SECONDS / 2.0);
        let half = fx.lights()[0].color[0];
        assert!((half - EXPLOSION_COLOR[0] * 0.5).abs() < 1e-4);
        fx.advance(EXPLOSION_SECONDS);
        assert!(fx.lights().is_empty());
    }

    #[test]
    fn pack_keeps_the_brightest() {
        let light = |radius| DynamicLight {
            origin: [0.0; 3],
            radius,
            color: [1.0; 3],
        };
        let lights: Vec<_> = (1..=6).map(|i| light(i as f32 * 100.0)).collect();
        let (packed, count) = pack(&lights);
        assert_eq!(count as usize, MAX_LIGHTS);
        assert_eq!(packed[0][3], 600.0);
        assert_eq!(packed[(MAX_LIGHTS - 1) * 2][3], 300.0);
    }
}
