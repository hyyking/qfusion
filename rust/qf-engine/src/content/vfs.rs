//! Minimal virtual filesystem over pk3 (zip) archives and loose directories.
//!
//! Lookups are case-insensitive and use `/` separators. Sources mounted later
//! override earlier ones, matching the legacy "last pk3 wins" search order.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zip::ZipArchive;

#[derive(Debug)]
pub enum VfsError {
    Io(String),
    Zip(String),
    NotFound(String),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::Zip(e) => write!(f, "archive error: {e}"),
            Self::NotFound(name) => write!(f, "{name}: not found in any mounted source"),
        }
    }
}

impl std::error::Error for VfsError {}

enum Source {
    Pk3(Mutex<ZipArchive<BufReader<File>>>),
    Dir(PathBuf),
}

#[derive(Clone, Copy)]
struct Location {
    source: usize,
    /// Zip entry index; unused for directories.
    entry: usize,
}

#[derive(Default)]
pub struct Vfs {
    sources: Vec<(String, Source)>,
    index: HashMap<String, Location>,
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches('/')
        .to_lowercase()
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mounts a pk3 archive; its entries override earlier mounts.
    pub fn mount_pk3(&mut self, path: &Path) -> Result<usize, VfsError> {
        let file =
            File::open(path).map_err(|e| VfsError::Io(format!("{}: {e}", path.display())))?;
        let mut zip = ZipArchive::new(BufReader::new(file))
            .map_err(|e| VfsError::Zip(format!("{}: {e}", path.display())))?;
        let source = self.sources.len();
        let mut count = 0;
        for entry in 0..zip.len() {
            let Ok(file) = zip.by_index_raw(entry) else {
                continue;
            };
            if file.is_dir() {
                continue;
            }
            self.index
                .insert(normalize(file.name()), Location { source, entry });
            count += 1;
        }
        self.sources
            .push((path.display().to_string(), Source::Pk3(Mutex::new(zip))));
        Ok(count)
    }

    /// Mounts every `*.pk3` directly inside `dir` in alphabetical order, as the legacy
    /// filesystem does, and returns the number of archives mounted.
    pub fn mount_pk3_dir(&mut self, dir: &Path) -> Result<usize, VfsError> {
        let mut paks: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| VfsError::Io(format!("{}: {e}", dir.display())))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pk3"))
            })
            .collect();
        paks.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
        for pak in &paks {
            self.mount_pk3(pak)?;
        }
        Ok(paks.len())
    }

    /// Mounts a loose directory tree; files are resolved on demand.
    pub fn mount_dir(&mut self, dir: &Path) {
        self.sources
            .push((dir.display().to_string(), Source::Dir(dir.to_path_buf())));
    }

    pub fn exists(&self, name: &str) -> bool {
        let key = normalize(name);
        self.loose_hit(&key).is_some() || self.index.contains_key(&key)
    }

    /// Latest-mounted loose file for `key` with its source index.
    fn loose_hit(&self, key: &str) -> Option<(usize, PathBuf)> {
        if key.split('/').any(|part| part == "..") {
            return None;
        }
        self.sources
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, (_, source))| match source {
                Source::Dir(root) => {
                    let path = root.join(key);
                    path.is_file().then_some((i, path))
                }
                Source::Pk3(_) => None,
            })
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>, VfsError> {
        let key = normalize(name);
        let pk3 = self.index.get(&key).copied();
        // Later mounts win, so a loose directory mounted after the pk3s overrides them.
        if let Some((source, path)) = self.loose_hit(&key)
            && pk3.is_none_or(|location| source > location.source)
        {
            return std::fs::read(&path)
                .map_err(|e| VfsError::Io(format!("{}: {e}", path.display())));
        }
        let location = pk3.ok_or_else(|| VfsError::NotFound(name.to_string()))?;
        let Source::Pk3(zip) = &self.sources[location.source].1 else {
            return Err(VfsError::NotFound(name.to_string()));
        };
        let mut zip = zip.lock().unwrap_or_else(|e| e.into_inner());
        let mut file = zip
            .by_index(location.entry)
            .map_err(|e| VfsError::Zip(format!("{name}: {e}")))?;
        let mut data = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut data)
            .map_err(|e| VfsError::Io(format!("{name}: {e}")))?;
        Ok(data)
    }

    /// Lists normalized entry names below `prefix` across all pk3 mounts.
    pub fn list(&self, prefix: &str) -> Vec<String> {
        let prefix = normalize(prefix);
        let mut names: Vec<String> = self
            .index
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect();
        names.sort();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("qf-vfs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_pk3(path: &Path, files: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn later_archive_overrides_and_lookup_ignores_case() {
        let dir = temp_dir("override");
        write_pk3(&dir.join("a.pk3"), &[("sounds/x.ogg", b"old")]);
        write_pk3(&dir.join("b.pk3"), &[("Sounds/X.ogg", b"new")]);
        let mut vfs = Vfs::new();
        assert_eq!(vfs.mount_pk3_dir(&dir).unwrap(), 2);
        assert_eq!(vfs.read("SOUNDS/x.OGG").unwrap(), b"new");
        assert!(matches!(
            vfs.read("sounds/y.ogg"),
            Err(VfsError::NotFound(_))
        ));
        assert_eq!(vfs.list("sounds/"), vec!["sounds/x.ogg"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loose_directory_mounted_last_overrides_archive() {
        let dir = temp_dir("loose");
        write_pk3(&dir.join("a.pk3"), &[("sounds/x.ogg", b"pk3")]);
        let loose = dir.join("loose");
        std::fs::create_dir_all(loose.join("sounds")).unwrap();
        std::fs::write(loose.join("sounds/x.ogg"), b"loose").unwrap();
        let mut vfs = Vfs::new();
        vfs.mount_pk3(&dir.join("a.pk3")).unwrap();
        vfs.mount_dir(&loose);
        assert_eq!(vfs.read("sounds/x.ogg").unwrap(), b"loose");
        assert!(vfs.read("../a.pk3").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
