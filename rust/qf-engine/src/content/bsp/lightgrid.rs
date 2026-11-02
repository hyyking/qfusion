//! Baked light grid (`lightgrid` lump) used to light dynamic models.
//!
//! Follows `Mod_LoadLightgrid` and `R_LightForOrigin` of the legacy renderer for the
//! `FBSP`/`IBSP` layout: 8-byte cells (ambient, diffuse, lat/long direction), an identity
//! light array and only light style 0. The legacy loader copies the stored diffuse
//! colour into both ambient and diffuse, which is kept here.

use std::f32::consts::TAU;

const CELL_SIZE: usize = 8;
const DEFAULT_GRID_SIZE: [f32; 3] = [64.0, 64.0, 128.0];

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    color: [u8; 3],
    direction: [u8; 2],
}

/// Light sampled at a point; colours are in 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridLight {
    pub direction: [f32; 3],
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
}

impl Default for GridLight {
    /// What the legacy renderer uses without a grid: a fixed direction, flat light.
    fn default() -> Self {
        Self {
            direction: normalize([0.1, 0.2, 0.7]),
            ambient: [0.55; 3],
            diffuse: [0.45; 3],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct LightGrid {
    cells: Vec<Cell>,
    size: [f32; 3],
    mins: [f32; 3],
    bounds: [i32; 3],
}

impl LightGrid {
    /// Decodes the lump. `world_mins`/`world_maxs` are the bounds of model 0 and
    /// `grid_size` the worldspawn `gridsize` key, if any.
    pub fn parse(
        lump: &[u8],
        world_mins: [f32; 3],
        world_maxs: [f32; 3],
        grid_size: Option<[f32; 3]>,
    ) -> Self {
        let cells: Vec<Cell> = lump
            .chunks_exact(CELL_SIZE)
            .map(|c| Cell {
                color: [c[3], c[4], c[5]],
                direction: [c[6], c[7]],
            })
            .collect();
        let size = match grid_size {
            Some(s) if s.iter().all(|&v| v >= 1.0) => s,
            _ => DEFAULT_GRID_SIZE,
        };
        let mut mins = [0.0; 3];
        let mut bounds = [1; 3];
        for axis in 0..3 {
            mins[axis] = size[axis] * ((world_mins[axis] + 1.0) / size[axis]).ceil();
            let max = size[axis] * ((world_maxs[axis] - 1.0) / size[axis]).floor();
            bounds[axis] = (((max - mins[axis]) / size[axis]) as i32).max(0) + 1;
        }
        Self {
            cells,
            size,
            mins,
            bounds,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Trilinear light at `origin` (`R_LightForOrigin` without dynamic lights).
    pub fn sample(&self, origin: [f32; 3]) -> GridLight {
        if self.cells.is_empty() {
            return GridLight::default();
        }
        let mut base = [0i32; 3];
        let mut frac = [0.0f32; 3];
        for axis in 0..3 {
            let v = (origin[axis] - self.mins[axis]) / self.size[axis];
            base[axis] = v as i32;
            frac[axis] = v - v.floor();
        }
        let row = self.bounds[0];
        let layer = self.bounds[0] * self.bounds[1];
        let first = base[2] * layer + base[1] * row + base[0];
        let last = self.cells.len() as i32 - 1;
        let at = |index: i32| self.cells[index.clamp(0, last) as usize];
        let mut direction = [0.0; 3];
        let mut color = [0.0; 3];
        for corner in 0..8 {
            let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, corner >> 2);
            let weight = [(dx, 0), (dy, 1), (dz, 2)]
                .iter()
                .map(|&(hi, axis)| {
                    if hi == 1 {
                        frac[axis]
                    } else {
                        1.0 - frac[axis]
                    }
                })
                .product::<f32>();
            let cell = at(first + dx + dy * row + dz * layer);
            let d = lat_long_to_normal(cell.direction);
            for i in 0..3 {
                direction[i] += d[i] * weight;
                color[i] += f32::from(cell.color[i]) * weight;
            }
        }
        let scale = 1.0 / 255.0;
        let color = clamp_color(color.map(|c| c * scale));
        GridLight {
            direction: normalize(direction),
            ambient: color,
            diffuse: color,
        }
    }
}

/// `ColorNormalize`: scale down so the largest component is at most 1.
fn clamp_color(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    if max > 1.0 { c.map(|v| v / max) } else { c }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        v.map(|c| c / len)
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// `R_LatLongToNorm` with the legacy 256-entry sine table.
fn lat_long_to_normal(latlong: [u8; 2]) -> [f32; 3] {
    let sin = |i: u8| (f32::from(i) * TAU / 256.0).sin();
    let (a, b) = (latlong[0], latlong[1]);
    let (cos_a, sin_a) = (sin(a.wrapping_add(64)), sin(a));
    let (cos_b, sin_b) = (sin(b.wrapping_add(64)), sin(b));
    [cos_b * sin_a, sin_b * sin_a, cos_a]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(color: [u8; 3], direction: [u8; 2]) -> [u8; 8] {
        [
            0,
            0,
            0,
            color[0],
            color[1],
            color[2],
            direction[0],
            direction[1],
        ]
    }

    #[test]
    fn empty_grid_uses_default_light() {
        let grid = LightGrid::parse(&[], [0.0; 3], [64.0; 3], None);
        assert!(grid.is_empty());
        assert_eq!(grid.sample([1.0; 3]), GridLight::default());
    }

    #[test]
    fn latlong_zero_points_up_and_quarter_turn_points_along_x_axis_pair() {
        let up = lat_long_to_normal([0, 0]);
        assert!((up[2] - 1.0).abs() < 1e-5);
        let side = lat_long_to_normal([64, 0]);
        assert!((side[0] - 1.0).abs() < 1e-5 && side[2].abs() < 1e-5);
    }

    #[test]
    fn interpolates_between_cells_and_clamps_outside() {
        // World spanning two cells along x: grid mins 64, bounds [2, 1, 1] with size 64.
        let mut lump = Vec::new();
        lump.extend(cell([0, 0, 0], [0, 0]));
        lump.extend(cell([200, 100, 0], [0, 0]));
        let grid = LightGrid::parse(
            &lump,
            [0.0, 0.0, 0.0],
            [200.0, 100.0, 100.0],
            Some([64.0, 64.0, 128.0]),
        );
        assert_eq!(grid.bounds, [3, 1, 1]);
        let mid = grid.sample([96.0, 64.0, 128.0]);
        assert!((mid.ambient[0] - 100.0 / 255.0).abs() < 1e-4);
        let far = grid.sample([5000.0, 5000.0, 5000.0]);
        assert!(far.ambient.iter().all(|c| (0.0..=1.0).contains(c)));
    }
}
