//! Parses every md3 of the real game data (skipped when the install is absent; set `QF_BASEWSW`).

use std::path::PathBuf;

use qf_engine::content::md3;
use qf_engine::content::vfs::Vfs;

#[test]
fn parses_all_item_weapon_and_object_models() {
    let dir: PathBuf = std::env::var("QF_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into())
        .into();
    if !dir.is_dir() {
        eprintln!("skipped: {} not found", dir.display());
        return;
    }
    let mut vfs = Vfs::new();
    vfs.mount_pk3_dir(&dir).unwrap();
    let mut n = 0;
    for prefix in ["models/items/", "models/weapons/", "models/objects/"] {
        for name in vfs.list(prefix).into_iter().filter(|f| f.ends_with(".md3")) {
            let m = md3::parse(&vfs.read(&name).unwrap()).unwrap_or_else(|e| panic!("{name}: {e}"));
            // `*_hand` models are tag-only.
            assert!(!m.surfaces.is_empty() || name.ends_with("_hand.md3"), "{name}");
            n += 1;
        }
    }
    assert!(n > 50, "only {n} models found");
}
