//! Headless content audit: mounts a game directory plus each map pack, parses the shader
//! scripts, resolves every texture referenced by the BSP and reports what the client would
//! replace with the checker texture, the shader warnings (grouped by directive) and sounds
//! that cannot be found.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use qf_engine::content::bsp::Bsp;
use qf_engine::content::image::load_image;
use qf_engine::content::shader::{MapSource, ShaderLib};
use qf_engine::content::vfs::Vfs;
use qf_engine::sound::{
    SOUND_JUMPPAD, SOUND_LAUNCHPAD, SOUND_TELEPORT_IN, SOUND_TELEPORT_OUT, SOUND_WALLJUMP_FAILED,
    SpeakerSet,
};

const USAGE: &str = "usage: world-audit <game-dir> [--map <pack.pk3>]... [--verbose]\n\
    Without --map every map_*.pk3 in the game directory is audited.";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}\n{USAGE}");
        std::process::exit(1);
    }
}

fn sound_exists(vfs: &Vfs, name: &str) -> bool {
    let lower = name.replace('\\', "/").to_ascii_lowercase();
    let stem = lower
        .strip_suffix(".ogg")
        .or_else(|| lower.strip_suffix(".wav"))
        .unwrap_or(&lower);
    vfs.exists(&format!("{stem}.ogg")) || vfs.exists(&format!("{stem}.wav"))
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().ok_or("missing game dir")?);
    let (mut maps, mut verbose) = (Vec::<PathBuf>::new(), false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--map" => maps.push(args.next().ok_or("--map requires a value")?.into()),
            "--verbose" => verbose = true,
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    if maps.is_empty() {
        let mut found: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("map_") && n.ends_with(".pk3"))
            })
            .collect();
        found.sort();
        maps = found;
    }

    let mut totals = (0usize, 0usize, 0usize, 0usize);
    let mut by_directive: BTreeMap<String, usize> = BTreeMap::new();
    let mut all_missing: BTreeSet<String> = BTreeSet::new();
    let mut all_unreadable: BTreeSet<String> = BTreeSet::new();
    let mut all_sounds: BTreeSet<String> = BTreeSet::new();
    let mut unique_warnings: BTreeSet<String> = BTreeSet::new();
    for map in &maps {
        let mut vfs = Vfs::new();
        vfs.mount_pk3_dir(&dir).map_err(|e| e.to_string())?;
        vfs.mount_pk3(map).map_err(|e| e.to_string())?;
        let mut shaders = ShaderLib::new();
        let parsed = shaders.load_vfs(&vfs);
        let stem = map.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let bsp_name = format!("maps/{}.bsp", stem.trim_start_matches("map_"));
        let bsp = Bsp::parse(
            &vfs.read(&bsp_name)
                .map_err(|e| format!("{bsp_name}: {e}"))?,
        )
        .map_err(|e| format!("{bsp_name}: {e}"))?;

        let mut images = BTreeSet::new();
        for s in &bsp.shaders {
            let shader = shaders.resolve(&s.name);
            if shader.is_hidden() || shader.is_sky() {
                continue;
            }
            for stage in &shader.stages {
                match &stage.map {
                    MapSource::Image(n) => {
                        images.insert(n.clone());
                    }
                    MapSource::Anim { frames, .. } => images.extend(frames.iter().cloned()),
                    _ => {}
                }
            }
        }
        let (mut missing, mut unreadable) = (BTreeSet::new(), BTreeSet::new());
        for name in &images {
            match load_image(&vfs, name) {
                Ok(_) => {}
                Err(qf_engine::content::image::ImageError::NotFound(_)) => {
                    missing.insert(name.clone());
                }
                Err(e) => {
                    unreadable.insert(format!("{name}: {e}"));
                }
            }
        }

        let mut sounds: BTreeSet<String> = [
            SOUND_TELEPORT_IN,
            SOUND_TELEPORT_OUT,
            SOUND_JUMPPAD,
            SOUND_LAUNCHPAD,
            SOUND_WALLJUMP_FAILED,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        sounds.extend(
            SpeakerSet::from_entities(&bsp.parsed_entities)
                .speakers
                .into_iter()
                .map(|s| s.noise),
        );
        let missing_sounds: BTreeSet<String> = sounds
            .into_iter()
            .filter(|s| !sound_exists(&vfs, s))
            .collect();

        for w in &shaders.warnings {
            *by_directive.entry(warning_kind(w)).or_default() += 1;
            unique_warnings.insert(w.clone());
        }
        println!(
            "{stem}: {parsed} shaders, {} warnings, {} textures, {} missing, {} unreadable, {} missing sounds",
            shaders.warnings.len(),
            images.len(),
            missing.len(),
            unreadable.len(),
            missing_sounds.len()
        );
        if verbose {
            for m in &missing {
                println!("  missing texture: {m}");
            }
            for u in &unreadable {
                println!("  unreadable texture: {u}");
            }
            for s in &missing_sounds {
                println!("  missing sound: {s}");
            }
        }
        totals.0 += shaders.warnings.len();
        totals.1 += missing.len();
        totals.2 += unreadable.len();
        totals.3 += missing_sounds.len();
        all_missing.extend(missing);
        all_unreadable.extend(unreadable);
        all_sounds.extend(missing_sounds);
    }
    println!(
        "TOTAL over {} maps: warnings {} | missing textures {} | unreadable (checker) textures {} | missing sounds {}",
        maps.len(),
        totals.0,
        totals.1,
        totals.2,
        totals.3
    );
    println!(
        "unique: {} missing textures, {} unreadable textures, {} missing sounds",
        all_missing.len(),
        all_unreadable.len(),
        all_sounds.len()
    );
    for u in &all_unreadable {
        println!("  unreadable: {u}");
    }
    for s in &all_sounds {
        println!("  missing sound: {s}");
    }
    println!("warnings by kind (summed over maps; base scripts repeat per map):");
    if verbose {
        println!("unique warnings ({}):", unique_warnings.len());
        for w in &unique_warnings {
            println!("  {w}");
        }
        for m in &all_missing {
            println!("  missing texture: {m}");
        }
    }
    let mut kinds: Vec<_> = by_directive.into_iter().collect();
    kinds.sort_by_key(|k| std::cmp::Reverse(k.1));
    for (k, n) in kinds {
        println!("  {n:6} {k}");
    }
    Ok(())
}

/// Groups a warning message by its directive: `shader: <kind> ...`.
fn warning_kind(w: &str) -> String {
    match w.split_once("malformed ") {
        Some((_, rest)) => format!(
            "malformed {}",
            rest.split_whitespace().next().unwrap_or("?")
        ),
        None => match w.split_once("unknown directive `") {
            Some((_, rest)) => format!("unknown {}", rest.split('`').next().unwrap_or("?")),
            None => w.split_once(": ").map_or(w, |(_, m)| m).to_string(),
        },
    }
}
