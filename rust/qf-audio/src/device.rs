//! cpal output: owns the stream and runs the [`Mixer`] inside its callback.

use std::fmt;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::mixer::{self, Mixer, MixerHandle};

/// Largest slice rendered at once; bigger device buffers are processed in pieces so the
/// scratch buffer can be allocated up front.
const MAX_CHUNK_FRAMES: usize = 2048;

#[derive(Debug)]
pub enum AudioError {
    NoOutputDevice,
    Config(String),
    UnsupportedFormat(String),
    Stream(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutputDevice => write!(f, "no default audio output device available"),
            Self::Config(e) => write!(f, "could not query output configuration: {e}"),
            Self::UnsupportedFormat(s) => write!(f, "unsupported sample format: {s}"),
            Self::Stream(e) => write!(f, "could not build/start output stream: {e}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A running output stream. Dropping it stops playback.
pub struct AudioDevice {
    _stream: Stream,
    sample_rate: u32,
}

impl AudioDevice {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Opens the default output device and starts a stream mixing for the returned handle.
    pub fn start_default() -> Result<(Self, MixerHandle), AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let sample_rate = config.sample_rate;
        let (handle, mixer) = mixer::create(sample_rate);

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, mixer),
            SampleFormat::I16 => build::<i16>(&device, config, mixer),
            SampleFormat::U16 => build::<u16>(&device, config, mixer),
            other => return Err(AudioError::UnsupportedFormat(other.to_string())),
        }?;
        stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        Ok((
            Self {
                _stream: stream,
                sample_rate,
            },
            handle,
        ))
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut mixer: Mixer,
) -> Result<Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let mut scratch = vec![0.0f32; MAX_CHUNK_FRAMES * 2];
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for chunk in data.chunks_mut(channels * MAX_CHUNK_FRAMES) {
                    let frames = chunk.len() / channels;
                    mixer.render(&mut scratch[..frames * 2]);
                    write_frames(chunk, channels, &scratch[..frames * 2]);
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| AudioError::Stream(e.to_string()))
}

/// Converts interleaved stereo into the device layout: mono is downmixed and channels
/// beyond the first two stay silent.
fn write_frames<T: Copy + cpal::Sample + FromSample<f32>>(
    out: &mut [T],
    channels: usize,
    stereo: &[f32],
) {
    for (frame, pair) in out.chunks_exact_mut(channels).zip(stereo.chunks_exact(2)) {
        if channels == 1 {
            frame[0] = T::from_sample(0.5 * (pair[0] + pair[1]));
        } else {
            frame[0] = T::from_sample(pair[0]);
            frame[1] = T::from_sample(pair[1]);
            for extra in &mut frame[2..] {
                *extra = T::from_sample(0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_device_gets_downmix() {
        let mut out = [9.0f32; 2];
        write_frames(&mut out, 1, &[1.0, 0.0, 0.5, 0.5]);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn surround_device_keeps_front_pair_and_silences_the_rest() {
        let mut out = [9.0f32; 6];
        write_frames(&mut out, 6, &[0.25, -0.25]);
        assert_eq!(out, [0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn integer_formats_convert() {
        let mut out = [0i16; 2];
        write_frames(&mut out, 2, &[1.0, -1.0]);
        assert!(out[0] > 32000 && out[1] < -32000);
    }
}
