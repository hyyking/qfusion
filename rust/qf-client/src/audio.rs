//! Audio output via cpal. Owns one persistent output stream; `play_beep`
//! restarts a pre-generated test tone from the callback side.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, Stream, StreamConfig};

use crate::tone;

const IDLE: usize = usize::MAX;

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

pub struct AudioOut {
    _stream: Stream,
    cursor: Arc<AtomicUsize>,
}

impl AudioOut {
    pub fn new() -> Result<Self, AudioError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(AudioError::NoOutputDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|e| AudioError::Config(e.to_string()))?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();

        let cursor = Arc::new(AtomicUsize::new(IDLE));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, &cursor),
            SampleFormat::I16 => build::<i16>(&device, config, &cursor),
            SampleFormat::U16 => build::<u16>(&device, config, &cursor),
            other => return Err(AudioError::UnsupportedFormat(other.to_string())),
        }?;
        stream
            .play()
            .map_err(|e| AudioError::Stream(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            cursor,
        })
    }

    /// Restart the test tone from the beginning.
    pub fn play_beep(&self) {
        self.cursor.store(0, Ordering::Release);
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    cursor: &Arc<AtomicUsize>,
) -> Result<Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let beep = tone::sine_beep(config.sample_rate, 660.0, 0.3, 0.25);
    let cursor = Arc::clone(cursor);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let start = cursor.load(Ordering::Acquire);
                let mut pos = start;
                for frame in data.chunks_mut(channels) {
                    let v = if pos < beep.len() {
                        let s = beep[pos];
                        pos += 1;
                        s
                    } else {
                        0.0
                    };
                    frame.fill(T::from_sample(v));
                }
                let next = if pos >= beep.len() { IDLE } else { pos };
                // Only publish if no new play request arrived meanwhile.
                let _ = cursor.compare_exchange(start, next, Ordering::AcqRel, Ordering::Acquire);
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| AudioError::Stream(e.to_string()))
}
