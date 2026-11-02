//! Minimal column-major 4x4 matrix helpers for the BSP preview.

pub type Mat4 = [[f32; 4]; 4];

/// `a * b` (apply `b` first, then `a`).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

pub fn look_at(eye: [f32; 3], target: [f32; 3], up: [f32; 3]) -> Mat4 {
    let forward = normalize(sub(target, eye));
    let right = normalize(cross(forward, up));
    let up = cross(right, forward);
    [
        [right[0], up[0], -forward[0], 0.0],
        [right[1], up[1], -forward[1], 0.0],
        [right[2], up[2], -forward[2], 0.0],
        [-dot(right, eye), -dot(up, eye), dot(forward, eye), 1.0],
    ]
}

/// Right-handed reversed-Z perspective: the near plane maps to depth 1 and the far plane to 0,
/// which spreads float depth precision evenly over distance.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let mut m = [[0.0; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = near / (far - near);
    m[2][3] = -1.0;
    m[3][2] = near * far / (far - near);
    m
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

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt();
    [v[0] / length, v[1] / length, v[2] / length]
}

#[cfg(test)]
fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (r, v) in out.iter_mut().enumerate() {
        *v = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn look_at_places_target_on_negative_view_z() {
        let m = look_at([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let p = transform_point(&m, [0.0, 0.0, 0.0]);
        assert!(
            close(p[0], 0.0) && close(p[1], 0.0) && close(p[2], -5.0),
            "{p:?}"
        );
    }

    #[test]
    fn perspective_maps_near_and_far_to_depth_range() {
        let m = perspective(1.0, 1.0, 0.1, 100.0);
        let depth = |z: f32| {
            let p = transform_point(&m, [0.0, 0.0, z]);
            p[2] / p[3]
        };
        assert!(close(depth(-0.1), 1.0));
        assert!(close(depth(-100.0), 0.0));
    }
}
