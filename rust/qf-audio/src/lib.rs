//! qfusion game audio: sound decoding and caching, a real-time mixer, legacy-compatible
//! spatialization, and the cpal output device. The simulation side (`qf-engine`) only
//! emits [`qf_engine::sound::SoundEvent`]s; [`system::AudioSystem`] plays them.

pub mod cache;
pub mod decode;
pub mod device;
pub mod mixer;
pub mod spatial;
pub mod system;

pub use device::{AudioDevice, AudioError};
pub use system::AudioSystem;
