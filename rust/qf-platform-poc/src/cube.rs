//! Static colored-cube geometry: 24 vertices (4 per face, flat colors).

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

const FACES: [([[f32; 3]; 4], [f32; 3]); 6] = [
    (
        [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
        [0.9, 0.2, 0.2],
    ),
    (
        [
            [1., -1., -1.],
            [-1., -1., -1.],
            [-1., 1., -1.],
            [1., 1., -1.],
        ],
        [0.2, 0.9, 0.2],
    ),
    (
        [[1., -1., 1.], [1., -1., -1.], [1., 1., -1.], [1., 1., 1.]],
        [0.2, 0.3, 0.9],
    ),
    (
        [
            [-1., -1., -1.],
            [-1., -1., 1.],
            [-1., 1., 1.],
            [-1., 1., -1.],
        ],
        [0.9, 0.9, 0.2],
    ),
    (
        [[-1., 1., 1.], [1., 1., 1.], [1., 1., -1.], [-1., 1., -1.]],
        [0.2, 0.9, 0.9],
    ),
    (
        [
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., -1., 1.],
            [-1., -1., 1.],
        ],
        [0.9, 0.2, 0.9],
    ),
];

pub fn vertices() -> Vec<Vertex> {
    FACES
        .iter()
        .flat_map(|(corners, color)| {
            corners.iter().map(|&position| Vertex {
                position,
                color: *color,
            })
        })
        .collect()
}

pub fn indices() -> Vec<u16> {
    (0..FACES.len() as u16)
        .flat_map(|f| {
            let b = f * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(vertices().len(), 24);
        assert_eq!(indices().len(), 36);
    }

    #[test]
    fn indices_in_bounds() {
        let n = vertices().len() as u16;
        assert!(indices().iter().all(|&i| i < n));
    }

    #[test]
    fn faces_wind_counter_clockwise_outward() {
        let v = vertices();
        for tri in indices().chunks(3) {
            let p: Vec<[f32; 3]> = tri.iter().map(|&i| v[i as usize].position).collect();
            let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            // Cube is centered on the origin, so the outward direction is the centroid.
            let c: [f32; 3] = std::array::from_fn(|i| (p[0][i] + p[1][i] + p[2][i]) / 3.0);
            let dot: f32 = (0..3).map(|i| n[i] * c[i]).sum();
            assert!(dot > 0.0, "triangle {tri:?} faces inward");
        }
    }
}
