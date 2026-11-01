//! Pure test-tone generation (no audio device involved).

/// Mono sine with short linear attack/release to avoid clicks.
pub fn sine_beep(sample_rate: u32, freq_hz: f32, duration_secs: f32, amplitude: f32) -> Vec<f32> {
    let n = (sample_rate as f32 * duration_secs) as usize;
    let fade = ((sample_rate as f32 * 0.01) as usize).min(n / 2).max(1);
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let env = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
            (std::f32::consts::TAU * freq_hz * t).sin() * amplitude * env
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_matches_duration() {
        assert_eq!(sine_beep(48_000, 440.0, 0.25, 0.5).len(), 12_000);
    }

    #[test]
    fn amplitude_is_bounded_and_nonsilent() {
        let s = sine_beep(44_100, 440.0, 0.3, 0.4);
        let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.3 && peak <= 0.4, "peak {peak}");
    }

    #[test]
    fn fades_at_both_ends() {
        let s = sine_beep(48_000, 440.0, 0.2, 0.5);
        assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 0.01);
    }

    #[test]
    fn zero_duration_is_empty() {
        assert!(sine_beep(48_000, 440.0, 0.0, 0.5).is_empty());
    }
}
