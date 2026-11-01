//! Quadratic Bezier patch tessellation and collision facets.
//!
//! Ports `source/common/helpers/patch.cpp` (flatness, evaluation, linear
//! row/column removal) and `CM_CreatePatch`/`CM_CreateFacetFromPoints` from
//! `source/common/facilities/cm_q3bsp.cpp`. Everything here is GPU independent
//! and deterministic.

use super::Plane;

/// Legacy `SUBDIVISIONS_DEFAULT` (the `r_subdivisions` default) used for render meshes.
pub const RENDER_SUBDIVISIONS: f32 = 5.0;
/// Legacy `CM_SUBDIV_LEVEL`, the flatness threshold for collision grids.
pub const COLLISION_SUBDIVISIONS: f32 = 16.0;
/// Upper bound on the flatness exponent (`1 << flat` steps per patch cell).
/// Legacy has no bound; this only guards against NaN/huge malformed patches.
pub const MAX_FLATNESS: u32 = 6;

const MAX_FACET_PLANES: usize = 32;
const PLANE_NORMAL_EPSILON: f32 = 0.00001;
const PLANE_DIST_EPSILON: f32 = 0.01;

/// True for control-point grids the legacy evaluator can address safely
/// (odd dimensions of at least 3 backed by enough vertices).
pub fn valid_control_points(cp: [i32; 2], num_verts: i32) -> bool {
    cp[0] >= 3
        && cp[1] >= 3
        && cp[0] % 2 == 1
        && cp[1] % 2 == 1
        && (cp[0] as i64) * (cp[1] as i64) <= num_verts as i64
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn avg(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        (a[0] + b[0]) * 0.5,
        (a[1] + b[1]) * 0.5,
        (a[2] + b[2]) * 0.5,
    ]
}

/// Normalizes in place and returns the original length (0 leaves the vector untouched).
pub fn normalize(v: &mut [f32; 3]) -> f32 {
    let length = dot(*v, *v).sqrt();
    if length > 0.0 {
        let inverse = 1.0 / length;
        v.iter_mut().for_each(|c| *c *= inverse);
    }
    length
}

fn flatness_test(maxflat2: f32, p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], depth: u32) -> u32 {
    let mut n = sub(p2, p0);
    if normalize(&mut n) == 0.0 {
        return 0;
    }
    let mut t = sub(p1, p0);
    let d = -dot(t, n);
    for k in 0..3 {
        t[k] += n[k] * d;
    }
    if dot(t, t) < maxflat2 || depth >= MAX_FLATNESS {
        return 0;
    }
    let v1 = avg(p1, p0);
    let v2 = avg(p2, p1);
    let v3 = avg(v1, v2);
    let ft0 = flatness_test(maxflat2, p0, v1, v3, depth + 1);
    let ft1 = flatness_test(maxflat2, v3, v2, p2, depth + 1);
    1 + ft0.max(ft1)
}

/// Subdivision exponents `[u, v]` needed so every curve deviates less than `maxflat`.
pub fn flatness(maxflat: f32, points: &[[f32; 3]], cp: [usize; 2]) -> [u32; 2] {
    let maxflat2 = maxflat * maxflat;
    let mut flat = [0u32; 2];
    let w = cp[0];
    let mut v = 0;
    while v + 1 < cp[1] {
        let mut u = 0;
        while u + 1 < cp[0] {
            let p = v * w + u;
            for row in 0..3 {
                let b = p + row * w;
                let i = flatness_test(maxflat2, points[b], points[b + 1], points[b + 2], 0);
                flat[0] = flat[0].max(i);
            }
            for col in 0..3 {
                let b = p + col;
                let i = flatness_test(maxflat2, points[b], points[b + w], points[b + 2 * w], 0);
                flat[1] = flat[1].max(i);
            }
            u += 2;
        }
        v += 2;
    }
    flat
}

fn bezier<const N: usize>(t: f32, p0: &[f32; N], p1: &[f32; N], p2: &[f32; N]) -> [f32; N] {
    let qt = t * t;
    let dt = 2.0 * t;
    let tt = 1.0 - dt + qt;
    let tt2 = dt - 2.0 * qt;
    std::array::from_fn(|k| p0[k] * tt + p1[k] * tt2 + p2[k] * qt)
}

/// Output grid dimensions for the given control grid and per-axis step counts.
pub fn grid_size(cp: [usize; 2], step: [usize; 2]) -> [usize; 2] {
    [(cp[0] / 2) * step[0] + 1, (cp[1] / 2) * step[1] + 1]
}

/// Evaluates a control grid (row-major, `cp[0]` wide) into a `grid_size` grid.
pub fn evaluate<const N: usize>(
    points: &[[f32; N]],
    cp: [usize; 2],
    step: [usize; 2],
) -> Vec<[f32; N]> {
    let patches = [cp[0] / 2, cp[1] / 2];
    let size = grid_size(cp, step);
    let pitch = size[0];
    let inv = [1.0 / step[0] as f32, 1.0 / step[1] as f32];
    let mut out = vec![[0.0; N]; size[0] * size[1]];

    for v in 0..patches[1] {
        let rows = if v < patches[1] - 1 {
            step[1]
        } else {
            step[1] + 1
        };
        for u in 0..patches[0] {
            let cols = if u < patches[0] - 1 {
                step[0]
            } else {
                step[0] + 1
            };
            let base = (v * cp[0] + u) * 2;
            let cell = |i: usize, j: usize| &points[base + j * cp[0] + i];
            let mut t = 0.0f32;
            for y in 0..rows {
                let v1 = bezier(t, cell(0, 0), cell(0, 1), cell(0, 2));
                let v2 = bezier(t, cell(1, 0), cell(1, 1), cell(1, 2));
                let v3 = bezier(t, cell(2, 0), cell(2, 1), cell(2, 2));
                let mut s = 0.0f32;
                let row_start = (v * step[1] + y) * pitch + u * step[0];
                for x in 0..cols {
                    out[row_start + x] = bezier(s, &v1, &v2, &v3);
                    s += inv[0];
                }
                t += inv[1];
            }
        }
    }
    out
}

fn linear_deviation(v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> f32 {
    let mut dir = sub(v1, v0);
    normalize(&mut dir);
    let along = dot(sub(v2, v0), dir);
    let proj = [
        v0[0] + dir[0] * along,
        v0[1] + dir[1] * along,
        v0[2] + dir[2] * along,
    ];
    let d = sub(v2, proj);
    dot(d, d)
}

/// Finds grid columns and rows that are straight lines between their neighbours
/// (`Patch_RemoveLinearColumnsRows`). Returns the kept column and row indices.
pub fn kept_rows_columns(verts: &[[f32; 3]], size: [usize; 2]) -> (Vec<usize>, Vec<usize>) {
    let [width, height] = size;
    let at = |col: usize, row: usize| verts[row * width + col];
    let mut cols = vec![0usize];
    for j in 1..width.saturating_sub(1) {
        let prev = *cols.last().expect("starts with column 0");
        let max = (0..height)
            .map(|i| linear_deviation(at(prev, i), at(j + 1, i), at(j, i)))
            .fold(0.0f32, f32::max);
        if max >= 0.01 {
            cols.push(j);
        }
    }
    if width > 1 {
        cols.push(width - 1);
    }
    let mut rows = vec![0usize];
    for j in 1..height.saturating_sub(1) {
        let prev = *rows.last().expect("starts with row 0");
        let max = cols
            .iter()
            .map(|&i| linear_deviation(at(i, prev), at(i, j + 1), at(i, j)))
            .fold(0.0f32, f32::max);
        if max >= 0.01 {
            rows.push(j);
        }
    }
    if height > 1 {
        rows.push(height - 1);
    }
    (cols, rows)
}

/// Keeps only the given columns and rows of a row-major grid `width` wide.
pub fn select<const N: usize>(
    grid: &[[f32; N]],
    width: usize,
    cols: &[usize],
    rows: &[usize],
) -> Vec<[f32; N]> {
    rows.iter()
        .flat_map(|&r| cols.iter().map(move |&c| grid[r * width + c]))
        .collect()
}

/// A tessellated patch: row-major grid of `size[0] x size[1]` vertices.
pub struct PatchGrid {
    pub size: [usize; 2],
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
}

/// Tessellates a patch with `flat` subdivision exponents, dropping linear rows/columns.
pub fn tessellate(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    cp: [usize; 2],
    flat: [u32; 2],
) -> PatchGrid {
    let step = [1usize << flat[0], 1usize << flat[1]];
    let size = grid_size(cp, step);
    let grid = evaluate(positions, cp, step);
    let (cols, rows) = kept_rows_columns(&grid, size);
    let mut normals_grid = select(&evaluate(normals, cp, step), size[0], &cols, &rows);
    for n in &mut normals_grid {
        normalize(n);
    }
    PatchGrid {
        size: [cols.len(), rows.len()],
        positions: select(&grid, size[0], &cols, &rows),
        normals: normals_grid,
        uvs: select(&evaluate(uvs, cp, step), size[0], &cols, &rows),
    }
}

/// Triangle indices for a grid, in the legacy winding.
pub fn grid_indices(size: [usize; 2]) -> Vec<u32> {
    let mut out = Vec::with_capacity(size[0].saturating_sub(1) * size[1].saturating_sub(1) * 6);
    for v in 0..size[1].saturating_sub(1) {
        for u in 0..size[0].saturating_sub(1) {
            let p = (v * size[0] + u) as u32;
            let w = size[0] as u32;
            out.extend_from_slice(&[p, p + w, p + 1, p + 1, p + w, p + w + 1]);
        }
    }
    out
}

/// A convex collision brush built from one quad or triangle of a patch grid.
#[derive(Debug, Clone)]
pub struct PatchFacet {
    pub planes: Vec<Plane>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

/// Collision data for one patch face.
#[derive(Debug, Clone)]
pub struct PatchCollide {
    pub face: usize,
    pub shader: usize,
    pub contents: i32,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub facets: Vec<PatchFacet>,
}

fn compare_planes(n1: [f32; 3], d1: f32, n2: [f32; 3], d2: f32) -> bool {
    (0..3).all(|k| (n1[k] - n2[k]).abs() < PLANE_NORMAL_EPSILON)
        && (d1 - d2).abs() < PLANE_DIST_EPSILON
}

fn snap_vector(normal: &mut [f32; 3]) {
    for i in 0..3 {
        if (normal[i] - 1.0).abs() < PLANE_NORMAL_EPSILON {
            *normal = [0.0; 3];
            normal[i] = 1.0;
            break;
        }
        if (normal[i] + 1.0).abs() < PLANE_NORMAL_EPSILON {
            *normal = [0.0; 3];
            normal[i] = -1.0;
            break;
        }
    }
}

fn snap_plane(normal: &mut [f32; 3], dist: &mut f32) {
    snap_vector(normal);
    let rounded = dist.round_ties_even();
    if (*dist - rounded).abs() < PLANE_DIST_EPSILON {
        *dist = rounded;
    }
}

/// `CM_CreateFacetFromPoints`: `verts` is a triangle or a coplanar quad.
fn create_facet(verts: &[[f32; 3]]) -> Option<PatchFacet> {
    let mut normal = cross(sub(verts[2], verts[0]), sub(verts[1], verts[0]));
    normalize(&mut normal);
    let mut dist = dot(verts[0], normal);
    if compare_planes(normal, dist, [0.0; 3], 0.0) {
        return None;
    }
    if verts.len() > 3 {
        let d = dot(verts[3], normal) - dist;
        if !(-0.1..=0.1).contains(&d) {
            return None;
        }
    }

    let mut planes: Vec<Plane> = Vec::with_capacity(MAX_FACET_PLANES);
    snap_plane(&mut normal, &mut dist);
    planes.push(Plane { normal, dist });

    let mut mins = [f32::MAX; 3];
    let mut maxs = [f32::MIN; 3];
    for v in verts {
        for k in 0..3 {
            mins[k] = mins[k].min(v[k]);
            maxs[k] = maxs[k].max(v[k]);
        }
    }

    for axis in 0..3 {
        for dir in [-1.0f32, 1.0] {
            if planes.iter().any(|p| p.normal[axis] == dir) {
                continue;
            }
            let mut n = [0.0; 3];
            n[axis] = dir;
            let dist = if dir == 1.0 { maxs[axis] } else { -mins[axis] };
            planes.push(Plane { normal: n, dist });
        }
    }

    'edges: for i in 0..verts.len() {
        let j = (i + 1) % verts.len();
        let mut edge = sub(verts[i], verts[j]);
        if normalize(&mut edge) < 0.5 {
            continue;
        }
        snap_vector(&mut edge);
        if edge.iter().any(|&c| c == 1.0 || c == -1.0) {
            continue;
        }
        for axis in 0..3 {
            for dir in [-1.0f32, 1.0] {
                let mut axial = [0.0; 3];
                axial[axis] = dir;
                let mut n = cross(edge, axial);
                if normalize(&mut n) < 0.5 {
                    continue;
                }
                let d = dot(verts[i], n);
                if planes
                    .iter()
                    .any(|p| compare_planes(p.normal, p.dist, n, d))
                {
                    continue;
                }
                let outer = verts
                    .iter()
                    .enumerate()
                    .all(|(k, v)| k == i || dot(*v, n) - d <= 0.1);
                if !outer {
                    continue;
                }
                planes.push(Plane { normal: n, dist: d });
                if planes.len() >= MAX_FACET_PLANES {
                    break 'edges;
                }
            }
        }
    }

    for p in &mut planes {
        snap_plane(&mut p.normal, &mut p.dist);
    }
    for k in 0..3 {
        mins[k] -= 1.0;
        maxs[k] += 1.0;
    }
    Some(PatchFacet { planes, mins, maxs })
}

/// `CM_CreatePatch`: builds collision facets for a control grid. `None` when no
/// facet could be created.
pub fn build_facets(
    control_points: &[[f32; 3]],
    cp: [usize; 2],
) -> Option<([f32; 3], [f32; 3], Vec<PatchFacet>)> {
    let flat = flatness(COLLISION_SUBDIVISIONS, control_points, cp);
    let step = [1usize << flat[0], 1usize << flat[1]];
    let size = grid_size(cp, step);
    let grid = evaluate(control_points, cp, step);
    let (cols, rows) = kept_rows_columns(&grid, size);
    let points = select(&grid, size[0], &cols, &rows);
    let size = [cols.len(), rows.len()];

    let mut facets = Vec::new();
    let mut mins = [f32::MAX; 3];
    let mut maxs = [f32::MIN; 3];
    for v in 0..size[1].saturating_sub(1) {
        for u in 0..size[0].saturating_sub(1) {
            let i = v * size[0] + u;
            let mut t = [
                points[i],
                points[i + size[0]],
                points[i + size[0] + 1],
                points[i + 1],
            ];
            for p in &t {
                for k in 0..3 {
                    mins[k] = mins[k].min(p[k]);
                    maxs[k] = maxs[k].max(p[k]);
                }
            }
            if let Some(f) = create_facet(&t) {
                facets.push(f);
                continue;
            }
            t[2] = t[3];
            facets.extend(create_facet(&t[..3]));
            t[0] = t[2];
            t[2] = points[i + size[0] + 1];
            facets.extend(create_facet(&t[..3]));
        }
    }
    if facets.is_empty() {
        return None;
    }
    for k in 0..3 {
        mins[k] -= 1.0;
        maxs[k] += 1.0;
    }
    Some((mins, maxs, facets))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x3 control grid, bulging up in the middle.
    fn arch() -> Vec<[f32; 3]> {
        let mut pts = Vec::new();
        for y in 0..3 {
            for x in 0..3 {
                let z = if x == 1 && y == 1 { 64.0 } else { 0.0 };
                pts.push([x as f32 * 64.0, y as f32 * 64.0, z]);
            }
        }
        pts
    }

    #[test]
    fn flat_patch_has_zero_flatness_and_collapses_to_one_cell() {
        let pts: Vec<[f32; 3]> = (0..9)
            .map(|i| [(i % 3) as f32 * 64.0, (i / 3) as f32 * 64.0, 0.0])
            .collect();
        assert_eq!(flatness(5.0, &pts, [3, 3]), [0, 0]);
        let grid = tessellate(&pts, &[[0.0, 0.0, 1.0]; 9], &[[0.0; 2]; 9], [3, 3], [2, 2]);
        assert_eq!(grid.size, [2, 2]);
    }

    #[test]
    fn evaluation_interpolates_corners_and_midpoint() {
        let pts = arch();
        let grid = evaluate(&pts, [3, 3], [4, 4]);
        assert_eq!(grid.len(), 25);
        assert_eq!(grid[0], pts[0]);
        assert_eq!(grid[4], pts[2]);
        assert_eq!(grid[24], pts[8]);
        // Centre of a biquadratic Bezier: 0.25 * 64 (z of the middle control point).
        assert_eq!(grid[12], [64.0, 64.0, 16.0]);
    }

    #[test]
    fn curved_patch_gets_subdivided_and_indices_stay_in_range() {
        let pts = arch();
        let flat = flatness(5.0, &pts, [3, 3]);
        assert!(flat[0] > 0 && flat[1] > 0);
        let grid = tessellate(&pts, &[[0.0, 0.0, 1.0]; 9], &[[0.0; 2]; 9], [3, 3], flat);
        let idx = grid_indices(grid.size);
        assert_eq!(idx.len(), (grid.size[0] - 1) * (grid.size[1] - 1) * 6);
        assert!(idx.iter().all(|&i| (i as usize) < grid.positions.len()));
        let again = tessellate(&pts, &[[0.0, 0.0, 1.0]; 9], &[[0.0; 2]; 9], [3, 3], flat);
        assert_eq!(grid.positions, again.positions);
    }

    #[test]
    fn facets_are_built_for_curved_patch() {
        let (mins, maxs, facets) = build_facets(&arch(), [3, 3]).expect("facets");
        assert!(!facets.is_empty());
        assert!(mins[2] <= -1.0 && maxs[2] >= 16.0);
        assert!(facets.iter().all(|f| f.planes.len() <= MAX_FACET_PLANES));
    }
}
