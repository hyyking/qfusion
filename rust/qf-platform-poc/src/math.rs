//! Minimal column-major 4x4 matrix helpers (just enough for the demo cube).

pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

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

pub fn rotation_y(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    let mut m = IDENTITY;
    m[0][0] = c;
    m[0][2] = -s;
    m[2][0] = s;
    m[2][2] = c;
    m
}

pub fn rotation_x(angle: f32) -> Mat4 {
    let (s, c) = angle.sin_cos();
    let mut m = IDENTITY;
    m[1][1] = c;
    m[1][2] = s;
    m[2][1] = -s;
    m[2][2] = c;
    m
}

pub fn translation(x: f32, y: f32, z: f32) -> Mat4 {
    let mut m = IDENTITY;
    m[3] = [x, y, z, 1.0];
    m
}

/// Right-handed perspective with wgpu clip-space depth range 0..1.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let mut m = [[0.0; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = far / (near - far);
    m[2][3] = -1.0;
    m[3][2] = near * far / (near - far);
    m
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
    fn identity_is_neutral() {
        let m = rotation_y(0.7);
        assert_eq!(mul(&IDENTITY, &m), m);
        assert_eq!(mul(&m, &IDENTITY), m);
    }

    #[test]
    fn rotation_y_quarter_turn_maps_x_to_minus_z() {
        let p = transform_point(&rotation_y(std::f32::consts::FRAC_PI_2), [1.0, 0.0, 0.0]);
        assert!(close(p[0], 0.0) && close(p[2], -1.0), "{p:?}");
    }

    #[test]
    fn mul_applies_right_operand_first() {
        let m = mul(
            &translation(0.0, 0.0, -5.0),
            &rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let p = transform_point(&m, [1.0, 0.0, 0.0]);
        assert!(close(p[0], 0.0) && close(p[2], -6.0), "{p:?}");
    }

    #[test]
    fn perspective_maps_near_and_far_to_depth_range() {
        let m = perspective(1.0, 1.0, 0.1, 100.0);
        let depth = |z: f32| {
            let p = transform_point(&m, [0.0, 0.0, z]);
            p[2] / p[3]
        };
        assert!(close(depth(-0.1), 0.0));
        assert!(close(depth(-100.0), 1.0));
    }
}
