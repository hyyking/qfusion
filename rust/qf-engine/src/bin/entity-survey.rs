//! Lists the classnames of brush entities (inline models) across official maps.
//!
//! Usage: `entity-survey [--dump] [basewsw-dir]` (default `WARSOW_BASEWSW`);
//! `--dump` also prints every `func_*` brush entity with its keys.

use std::collections::BTreeMap;

use qf_engine::content::bsp::{Bsp, read_pk3_entry};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dump = args.iter().any(|arg| arg == "--dump");
    args.retain(|arg| arg != "--dump");
    let base = args
        .first()
        .cloned()
        .or_else(|| std::env::var("WARSOW_BASEWSW").ok())
        .unwrap_or_else(|| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let Ok(dir) = std::fs::read_dir(&base) else {
        eprintln!("cannot read {base}");
        return;
    };
    let mut brush: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut maps = 0;
    for file in dir.flatten() {
        let name = file.file_name().to_string_lossy().into_owned();
        let Some(map) = name
            .strip_prefix("map_")
            .and_then(|n| n.strip_suffix(".pk3"))
        else {
            continue;
        };
        let Ok(data) = read_pk3_entry(&file.path(), &format!("maps/{map}.bsp")) else {
            continue;
        };
        let Ok(bsp) = Bsp::parse(&data) else { continue };
        maps += 1;
        let mut seen = std::collections::BTreeSet::new();
        for entity in &bsp.parsed_entities {
            let is_brush = entity
                .get("model")
                .is_some_and(|m| m.starts_with('*') && m != "*0");
            if !is_brush {
                continue;
            }
            let classname = entity.get("classname").unwrap_or("?").to_ascii_lowercase();
            if dump && classname.starts_with("func_") {
                println!("{map}: {:?}", entity.properties);
            }
            let entry = brush.entry(classname.clone()).or_default();
            entry.0 += 1;
            if seen.insert(classname) {
                entry.1 += 1;
            }
        }
    }
    println!("{maps} maps; brush entity classnames (count, maps):");
    for (classname, (count, in_maps)) in brush {
        println!("{classname}\t{count}\t{in_maps}");
    }
}
