use crate::simulation::pmove::Vec3;

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

pub fn distance(a: Vec3, b: Vec3) -> f32 {
    length(sub(a, b))
}

/// Axis-aligned box in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl Bounds {
    pub fn at(origin: Vec3, mins: Vec3, maxs: Vec3) -> Self {
        Self {
            mins: add(origin, mins),
            maxs: add(origin, maxs),
        }
    }

    pub fn center(&self) -> Vec3 {
        scale(add(self.mins, self.maxs), 0.5)
    }

    pub fn intersects(&self, other: &Bounds) -> bool {
        (0..3)
            .all(|axis| self.mins[axis] <= other.maxs[axis] && self.maxs[axis] >= other.mins[axis])
    }

    pub fn contains(&self, point: Vec3) -> bool {
        (0..3).all(|axis| point[axis] >= self.mins[axis] && point[axis] <= self.maxs[axis])
    }
}

/// Deterministic xorshift64* generator; all gameplay randomness comes from here so
/// a match replays identically from its seed.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// Raw generator state, for state hashing.
    pub fn state(&self) -> u64 {
        self.0
    }

    /// Independent generator for `(tick, purpose)` derived from the match seed, so adding
    /// or reordering consumers never shifts the numbers another consumer sees.
    pub fn substream(seed: u64, tick: u64, purpose: u32) -> Self {
        let mut z =
            seed ^ tick.rotate_left(32) ^ u64::from(purpose).wrapping_mul(0xA24B_AED4_963E_E407);
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Self::new(z ^ (z >> 31))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..count`; `count` must be non-zero.
    pub fn below(&mut self, count: usize) -> usize {
        (self.next_u64() >> 11) as usize % count
    }
}
