//! Decoded-sound cache over a virtual filesystem. Failures are never papered over: a
//! missing or undecodable sound is returned as an error every time it is requested.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use qf_engine::content::vfs::Vfs;

use crate::decode::{self, DecodeError};
use crate::mixer::Buffer;

/// Where sound files come from.
pub trait SoundSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

impl SoundSource for Vfs {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        Vfs::read(self, path).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SoundError {
    NotFound { name: String, tried: Vec<String> },
    Decode { path: String, error: DecodeError },
}

impl fmt::Display for SoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { name, tried } => {
                write!(f, "sound {name} not found (tried {})", tried.join(", "))
            }
            Self::Decode { path, error } => write!(f, "cannot decode {path}: {error}"),
        }
    }
}

impl std::error::Error for SoundError {}

/// Candidate files for a legacy sound name: an explicit extension is tried first and then
/// the other supported container (maps reference `.wav` names that ship as `.ogg`); a name
/// without extension tries `.ogg` before `.wav`.
pub fn candidate_paths(name: &str) -> Vec<String> {
    let name = name.trim().trim_start_matches('/');
    let lower = name.to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".ogg")
        .or_else(|| lower.strip_suffix(".wav"));
    match stem {
        Some(stem) => {
            let first = if lower.ends_with(".wav") {
                "wav"
            } else {
                "ogg"
            };
            let second = if first == "wav" { "ogg" } else { "wav" };
            vec![format!("{stem}.{first}"), format!("{stem}.{second}")]
        }
        None => vec![format!("{lower}.ogg"), format!("{lower}.wav")],
    }
}

pub struct SoundCache<S> {
    source: S,
    loaded: HashMap<String, Result<Arc<Buffer>, SoundError>>,
}

impl<S: SoundSource> SoundCache<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            loaded: HashMap::new(),
        }
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// Loads, decodes and caches `name`. The cache keeps the buffer alive for its own
    /// lifetime, so playing voices never hold the last reference.
    pub fn get(&mut self, name: &str) -> Result<Arc<Buffer>, SoundError> {
        let key = name.trim().to_ascii_lowercase();
        if let Some(entry) = self.loaded.get(&key) {
            return entry.clone();
        }
        let result = self.load(name);
        self.loaded.insert(key, result.clone());
        result
    }

    fn load(&self, name: &str) -> Result<Arc<Buffer>, SoundError> {
        let tried = candidate_paths(name);
        for path in &tried {
            let Some(data) = self.source.read(path) else {
                continue;
            };
            let pcm = decode::decode(&data).map_err(|error| SoundError::Decode {
                path: path.clone(),
                error,
            })?;
            return Ok(Arc::new(Buffer {
                name: path.clone(),
                sample_rate: pcm.sample_rate,
                channels: pcm.channels,
                samples: pcm.samples.into(),
            }));
        }
        Err(SoundError::NotFound {
            name: name.to_string(),
            tried,
        })
    }

    pub fn len(&self) -> usize {
        self.loaded.values().filter(|r| r.is_ok()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::tests::make_wav;
    use std::cell::Cell;
    use std::collections::HashMap;

    #[derive(Default)]
    pub(crate) struct MemorySource {
        pub files: HashMap<String, Vec<u8>>,
        pub reads: Cell<usize>,
    }

    impl SoundSource for MemorySource {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.reads.set(self.reads.get() + 1);
            self.files.get(path).cloned()
        }
    }

    #[test]
    fn candidates_cover_both_containers() {
        assert_eq!(
            candidate_paths("sounds/a/b.wav"),
            ["sounds/a/b.wav", "sounds/a/b.ogg"]
        );
        assert_eq!(
            candidate_paths("Sounds/B"),
            ["sounds/b.ogg", "sounds/b.wav"]
        );
        assert_eq!(candidate_paths("/x.OGG"), ["x.ogg", "x.wav"]);
    }

    #[test]
    fn caches_loaded_sounds() {
        let mut src = MemorySource::default();
        src.files
            .insert("sounds/a.wav".into(), make_wav(8000, 1, &[1, 2, 3]));
        let mut cache = SoundCache::new(src);
        let a = cache.get("sounds/a").unwrap();
        let b = cache.get("SOUNDS/A").unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.source().reads.get(), 2, "ogg miss then wav hit, once");
        assert_eq!(
            (a.name.as_str(), a.frames(), cache.len()),
            ("sounds/a.wav", 3, 1)
        );
    }

    #[test]
    fn missing_sound_is_an_error_every_time() {
        let mut cache = SoundCache::new(MemorySource::default());
        for _ in 0..2 {
            let err = cache.get("sounds/none").unwrap_err();
            assert!(matches!(err, SoundError::NotFound { .. }));
            assert!(err.to_string().contains("sounds/none.ogg"));
        }
    }

    #[test]
    fn corrupt_file_is_a_decode_error_not_a_fallback() {
        let mut src = MemorySource::default();
        src.files
            .insert("sounds/bad.ogg".into(), b"garbage!".to_vec());
        src.files
            .insert("sounds/bad.wav".into(), make_wav(8000, 1, &[1]));
        let mut cache = SoundCache::new(src);
        assert!(matches!(
            cache.get("sounds/bad.ogg"),
            Err(SoundError::Decode { .. })
        ));
    }
}
