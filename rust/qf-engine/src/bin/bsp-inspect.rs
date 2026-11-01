use qf_engine::content::bsp::{Bsp, read_pk3_entry};
use std::path::Path;

const USAGE: &str =
    "usage: bsp-inspect <map.bsp | pack.pk3> [--entry maps/wbomb1.bsp] [--obj out.obj]";

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}\n{USAGE}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("missing input")?;
    let (mut entry, mut obj) = (None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--entry" => entry = Some(args.next().ok_or("--entry requires a value")?),
            "--obj" => obj = Some(args.next().ok_or("--obj requires a value")?),
            _ => return Err(format!("unknown argument {a}")),
        }
    }
    let path = Path::new(&input);
    let data = if input.to_lowercase().ends_with(".pk3") {
        let entry = entry.ok_or("--entry is required for pk3 input")?;
        read_pk3_entry(path, &entry)?
    } else {
        std::fs::read(path).map_err(|e| e.to_string())?
    };
    let bsp = Bsp::parse(&data).map_err(|e| e.to_string())?;
    println!("file bytes: {}", data.len());
    println!(
        "magic: {} version: {} format: {:?}",
        bsp.magic, bsp.version, bsp.format
    );
    println!("lumps:");
    for (i, l) in bsp.lumps.iter().enumerate() {
        println!(
            "  {i:2} {:<12} offset={:<9} len={}",
            qf_engine::content::bsp::LUMP_NAMES[i],
            l.offset,
            l.len
        );
    }
    println!(
        "shaders: {}  planes: {}  models: {}",
        bsp.shaders.len(),
        bsp.planes.len(),
        bsp.models.len()
    );
    println!(
        "vertices: {}  elements: {}  faces: {}",
        bsp.vertices.len(),
        bsp.elements.len(),
        bsp.faces.len()
    );
    println!(
        "entities: {} parsed, {} text bytes",
        bsp.parsed_entities.len(),
        bsp.entities.len()
    );
    println!(
        "collision: {} nodes, {} leafs, {} leaf-brush refs, {} brushes, {} brushsides",
        bsp.nodes.len(),
        bsp.leafs.len(),
        bsp.leaf_brushes.len(),
        bsp.brushes.len(),
        bsp.brush_sides.len()
    );
    if let Some(m) = bsp.models.first() {
        println!("world bounds (model 0): {:?} .. {:?}", m.mins, m.maxs);
    }
    let mesh = bsp.model_mesh(0).map_err(|e| e.to_string())?;
    println!(
        "world mesh: {} vertices, {} triangles, {} faces skipped (patch/flare/foliage)",
        mesh.positions.len(),
        mesh.indices.len() / 3,
        mesh.skipped_faces
    );
    if let Some(b) = mesh.bounds() {
        println!("mesh bounds: {:?} .. {:?}", b.0, b.1);
    }
    println!("diagnostics:");
    for d in &bsp.diagnostics {
        println!("  - {d}");
    }
    if let Some(out) = obj {
        std::fs::write(&out, mesh.to_obj()).map_err(|e| e.to_string())?;
        println!("wrote {out}");
    }
    Ok(())
}
