//! Pure spatialization math: legacy distance attenuation and stereo panning.

/// OpenAL reference distance used by the legacy sound system (`protocol.h`).
pub const REF_DISTANCE: f32 = 250.0;
/// OpenAL maximum distance used by the legacy sound system.
pub const MAX_DISTANCE: f32 = 8000.0;

/// `AL_INVERSE_DISTANCE_CLAMPED` with the legacy constants, as computed by
/// `calcSoundGainForDistanceAndAttenuation`. `rolloff` is the legacy attenuation value.
pub fn distance_gain(distance: f32, rolloff: f32) -> f32 {
    let distance = distance.clamp(REF_DISTANCE, MAX_DISTANCE);
    REF_DISTANCE / (REF_DISTANCE + rolloff.max(0.0) * (distance - REF_DISTANCE))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub origin: [f32; 3],
    /// Unit vector pointing to the listener's right ear.
    pub right: [f32; 3],
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            origin: [0.0; 3],
            right: [0.0, -1.0, 0.0],
        }
    }
}

impl Listener {
    /// Builds a listener from an eye position and yaw in degrees. Quake axes: yaw 0 looks
    /// along +x and the right-hand side is -y. Pitch and roll do not change which side a
    /// source is on and are ignored.
    pub fn from_yaw(origin: [f32; 3], yaw_degrees: f32) -> Self {
        let (sin, cos) = yaw_degrees.to_radians().sin_cos();
        Self {
            origin,
            right: [sin, -cos, 0.0],
        }
    }
}

/// Distance and signed pan (-1 hard left, +1 hard right) of a source.
pub fn distance_and_pan(listener: &Listener, source: [f32; 3]) -> (f32, f32) {
    let d = [
        source[0] - listener.origin[0],
        source[1] - listener.origin[1],
        source[2] - listener.origin[2],
    ];
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if distance < 1e-3 {
        return (0.0, 0.0);
    }
    let side =
        (d[0] * listener.right[0] + d[1] * listener.right[1] + d[2] * listener.right[2]) / distance;
    (distance, side.clamp(-1.0, 1.0))
}

/// Left/right gains for a pan in [-1, 1]. Centered sources keep unity gain in both
/// channels and the far channel fades with an equal-power curve, so a source directly to
/// one side is silent in the other ear.
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let quarter = std::f32::consts::FRAC_PI_2;
    let left = (pan.max(0.0) * quarter).cos();
    let right = ((-pan).max(0.0) * quarter).cos();
    (left, right)
}

/// Final per-channel gains of a spatial source before voice volume is applied.
pub fn spatial_gains(listener: &Listener, source: [f32; 3], rolloff: f32) -> (f32, f32) {
    let (distance, pan) = distance_and_pan(listener, source);
    let gain = distance_gain(distance, rolloff);
    let (l, r) = pan_gains(pan);
    (l * gain, r * gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn full_gain_within_reference_distance() {
        assert!(close(distance_gain(0.0, 1.0), 1.0));
        assert!(close(distance_gain(REF_DISTANCE, 10.0), 1.0));
    }

    #[test]
    fn matches_legacy_inverse_distance_formula() {
        // 250 / (250 + 1 * (750 - 250)) = 1/3
        assert!(close(distance_gain(750.0, 1.0), 1.0 / 3.0));
        // 250 / (250 + 3 * 1000)
        assert!(close(distance_gain(1250.0, 3.0), 250.0 / 3250.0));
    }

    #[test]
    fn gain_is_clamped_at_max_distance_and_zero_rolloff_is_flat() {
        assert!(close(
            distance_gain(1.0e6, 2.0),
            distance_gain(MAX_DISTANCE, 2.0)
        ));
        assert!(close(distance_gain(5000.0, 0.0), 1.0));
    }

    #[test]
    fn higher_attenuation_is_quieter() {
        assert!(distance_gain(2000.0, 0.5) > distance_gain(2000.0, 2.5));
    }

    #[test]
    fn pan_follows_listener_orientation() {
        // Facing +x: +y is to the left, -y to the right.
        let facing_x = Listener::from_yaw([0.0; 3], 0.0);
        assert!(close(
            distance_and_pan(&facing_x, [0.0, -100.0, 0.0]).1,
            1.0
        ));
        assert!(close(
            distance_and_pan(&facing_x, [0.0, 100.0, 0.0]).1,
            -1.0
        ));
        assert!(close(distance_and_pan(&facing_x, [100.0, 0.0, 0.0]).1, 0.0));
        // Turning 90 degrees left makes +x the right-hand side.
        let facing_y = Listener::from_yaw([0.0; 3], 90.0);
        assert!(close(distance_and_pan(&facing_y, [100.0, 0.0, 0.0]).1, 1.0));
    }

    #[test]
    fn coincident_source_is_centered() {
        let l = Listener::default();
        assert_eq!(distance_and_pan(&l, [0.0; 3]), (0.0, 0.0));
    }

    #[test]
    fn pan_gains_keep_center_and_silence_far_ear() {
        assert_eq!(pan_gains(0.0), (1.0, 1.0));
        let (l, r) = pan_gains(1.0);
        assert!(l < 1e-6 && close(r, 1.0));
        let (l, r) = pan_gains(-1.0);
        assert!(close(l, 1.0) && r < 1e-6);
        let (l, r) = pan_gains(0.5);
        assert!(l < r + 1e-6 && l > 0.0);
    }

    #[test]
    fn spatial_gains_combine_distance_and_pan() {
        let l = Listener::from_yaw([0.0; 3], 0.0);
        let (gl, gr) = spatial_gains(&l, [0.0, -750.0, 0.0], 1.0);
        assert!(gl < 1e-6);
        assert!(close(gr, 1.0 / 3.0));
    }
}
