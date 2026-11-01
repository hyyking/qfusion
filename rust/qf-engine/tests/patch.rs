use qf_engine::content::bsp::Bsp;

fn le(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// One FBSP world model holding a single 3x3 patch whose centre control point is
/// raised by `peak`, so the surface passes through z = peak / 4 at (64, 64).
fn patch_bsp(peak: f32, contents: i32, surface_flags: i32) -> Vec<u8> {
    let mut shader = vec![0u8; 72];
    shader[64..68].copy_from_slice(&le(surface_flags));
    shader[68..72].copy_from_slice(&le(contents));

    let mut verts = Vec::new();
    for y in 0..3 {
        for x in 0..3 {
            let z = if x == 1 && y == 1 { peak } else { 0.0 };
            let mut v = vec![0u8; 80];
            v[0..12].copy_from_slice(&f32s(&[x as f32 * 64.0, y as f32 * 64.0, z]));
            v[12..20].copy_from_slice(&f32s(&[x as f32 * 0.5, y as f32 * 0.5]));
            v[52..64].copy_from_slice(&f32s(&[0.0, 0.0, 1.0]));
            verts.extend(v);
        }
    }

    let mut face = vec![0u8; 148];
    for (o, v) in [(8, 2), (12, 0), (16, 9), (140, 3), (144, 3)] {
        face[o..o + 4].copy_from_slice(&le(v));
    }
    face[104..116].copy_from_slice(&f32s(&[0.0, 0.0, 0.0]));
    face[116..128].copy_from_slice(&f32s(&[128.0, 128.0, 64.0]));

    let mut model = f32s(&[0.0, 0.0, 0.0, 128.0, 128.0, 64.0]);
    for v in [0, 1, 0, 0] {
        model.extend(le(v));
    }
    let mut plane = f32s(&[0.0, 0.0, 1.0]);
    plane.extend(f32s(&[10000.0]));
    let mut node = vec![0u8; 36];
    node[4..8].copy_from_slice(&le(-1));
    node[8..12].copy_from_slice(&le(-1));
    let mut leaf = vec![0u8; 48];
    leaf[36..40].copy_from_slice(&le(1));

    let lumps: Vec<(usize, Vec<u8>)> = vec![
        (0, b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec()),
        (1, shader),
        (2, plane),
        (3, node),
        (4, leaf),
        (5, le(0).to_vec()),
        (7, model),
        (10, verts),
        (13, face),
    ];
    let header_len = 8 + 18 * 8;
    let mut out = b"FBSP".to_vec();
    out.extend(le(1));
    let mut table = vec![(0i32, 0i32); 18];
    let mut body: Vec<u8> = Vec::new();
    for (i, data) in &lumps {
        table[*i] = ((header_len + body.len()) as i32, data.len() as i32);
        body.extend(data);
    }
    for (offset, len) in table {
        out.extend(le(offset));
        out.extend(le(len));
    }
    out.extend(body);
    out
}

const POINT: ([f32; 3], [f32; 3]) = ([0.0; 3], [0.0; 3]);

#[test]
fn patch_is_tessellated_into_the_world_mesh() {
    let bsp = Bsp::parse(&patch_bsp(64.0, 1, 0)).unwrap();
    let mesh = bsp.model_mesh(0).unwrap();
    assert_eq!(mesh.skipped_faces, 0);
    assert!(mesh.patch_triangles > 2);
    assert_eq!(mesh.indices.len() / 3, mesh.patch_triangles);
    assert_eq!(mesh.triangle_shaders.len(), mesh.patch_triangles);
    assert!(
        mesh.indices
            .iter()
            .all(|&i| (i as usize) < mesh.positions.len())
    );
    let (lo, hi) = mesh.bounds().unwrap();
    assert_eq!((lo, hi[0], hi[1]), ([0.0; 3], 128.0, 128.0));
    // Quadratic Bezier peak: 0.25 * 64.
    assert!((hi[2] - 16.0).abs() < 1e-3, "{hi:?}");
    assert!(mesh.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-5));
}

#[test]
fn tessellation_is_deterministic() {
    let data = patch_bsp(64.0, 1, 0);
    let a = Bsp::parse(&data).unwrap().model_mesh(0).unwrap();
    let b = Bsp::parse(&data).unwrap().model_mesh(0).unwrap();
    assert_eq!(a.positions, b.positions);
    assert_eq!(a.indices, b.indices);
}

#[test]
fn flat_patch_collapses_to_two_triangles() {
    let bsp = Bsp::parse(&patch_bsp(0.0, 1, 0)).unwrap();
    assert_eq!(bsp.model_mesh(0).unwrap().patch_triangles, 2);
}

#[test]
fn point_trace_hits_the_curved_surface() {
    let bsp = Bsp::parse(&patch_bsp(64.0, 1, 0)).unwrap();
    assert_eq!(bsp.patches.len(), 1);
    assert!(!bsp.patches[0].facets.is_empty());
    let hit = bsp
        .trace(
            [64.0, 64.0, 100.0],
            [64.0, 64.0, -50.0],
            POINT.0,
            POINT.1,
            1,
        )
        .unwrap();
    let z = hit.end_position[2];
    assert!(hit.fraction < 1.0 && (z - 16.0).abs() < 1.0, "{hit:?}");
    assert_eq!(hit.contents, 1);
    let plane = hit.plane.unwrap();
    assert!(plane.normal[2] > 0.99);
    assert_eq!(
        hit,
        bsp.trace(
            [64.0, 64.0, 100.0],
            [64.0, 64.0, -50.0],
            POINT.0,
            POINT.1,
            1
        )
        .unwrap()
    );
}

#[test]
fn sloped_flank_has_a_tilted_normal() {
    let bsp = Bsp::parse(&patch_bsp(64.0, 1, 0)).unwrap();
    let hit = bsp
        .trace(
            [32.0, 64.0, 100.0],
            [32.0, 64.0, -50.0],
            POINT.0,
            POINT.1,
            1,
        )
        .unwrap();
    let plane = hit.plane.unwrap();
    assert!(hit.fraction < 1.0);
    assert!(plane.normal[0] < -0.1 && plane.normal[2] > 0.5, "{plane:?}");
}

#[test]
fn player_box_stands_on_the_patch() {
    let bsp = Bsp::parse(&patch_bsp(0.0, 1, 0)).unwrap();
    let hit = bsp
        .trace(
            [64.0, 64.0, 200.0],
            [64.0, 64.0, -100.0],
            [-16.0, -16.0, -24.0],
            [16.0, 16.0, 40.0],
            1,
        )
        .unwrap();
    assert!((hit.end_position[2] - 24.0).abs() < 0.2, "{hit:?}");
}

#[test]
fn trace_beside_the_patch_misses() {
    let bsp = Bsp::parse(&patch_bsp(64.0, 1, 0)).unwrap();
    let miss = bsp
        .trace(
            [300.0, 64.0, 100.0],
            [300.0, 64.0, -50.0],
            POINT.0,
            POINT.1,
            1,
        )
        .unwrap();
    assert_eq!(miss.fraction, 1.0);
}

#[test]
fn contents_mask_and_nonsolid_surfaces_are_respected() {
    let down = ([64.0, 64.0, 100.0], [64.0, 64.0, -50.0]);
    let bsp = Bsp::parse(&patch_bsp(64.0, 1, 0)).unwrap();
    assert_eq!(
        bsp.trace(down.0, down.1, POINT.0, POINT.1, 2)
            .unwrap()
            .fraction,
        1.0
    );
    let nonsolid = Bsp::parse(&patch_bsp(64.0, 1, 0x4000)).unwrap();
    assert!(nonsolid.patches.is_empty());
    assert_eq!(
        nonsolid
            .trace(down.0, down.1, POINT.0, POINT.1, 1)
            .unwrap()
            .fraction,
        1.0
    );
    let no_contents = Bsp::parse(&patch_bsp(64.0, 0, 0)).unwrap();
    assert!(no_contents.patches.is_empty());
}

#[test]
fn survey_maps_tessellate_and_collide() {
    let root = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let Ok(dir) = std::fs::read_dir(&root) else {
        eprintln!("SKIPPED: {root} not found; survey maps NOT tested");
        return;
    };
    let (mut maps, mut total_patch_faces, mut total_patch_tris, mut total_tris) = (0, 0, 0, 0);
    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "pk3") {
            continue;
        }
        let names: Vec<String> = {
            let file = std::fs::File::open(&path).unwrap();
            let zip = zip::ZipArchive::new(file).unwrap();
            zip.file_names()
                .filter(|n| n.starts_with("maps/") && n.ends_with(".bsp"))
                .map(String::from)
                .collect()
        };
        for name in names {
            let data = qf_engine::content::bsp::read_pk3_entry(&path, &name).unwrap();
            let bsp = Bsp::parse(&data).unwrap();
            let mesh = bsp.model_mesh(0).unwrap();
            assert!(
                mesh.positions.iter().flatten().all(|c| c.is_finite()),
                "{name}"
            );
            assert!(
                mesh.normals.iter().flatten().all(|c| c.is_finite()),
                "{name}"
            );
            assert!(
                mesh.indices
                    .iter()
                    .all(|&i| (i as usize) < mesh.positions.len())
            );
            let patch_faces = bsp.faces.iter().filter(|f| f.facetype == 2).count();
            println!(
                "{name}: patch faces {patch_faces}, collidable {}, tris {} (patch {})",
                bsp.patches.len(),
                mesh.indices.len() / 3,
                mesh.patch_triangles
            );
            if patch_faces > 0 {
                assert!(mesh.patch_triangles > 0, "{name}");
            }
            maps += 1;
            total_patch_faces += patch_faces;
            total_patch_tris += mesh.patch_triangles;
            total_tris += mesh.indices.len() / 3;
        }
    }
    println!(
        "{maps} maps: {total_patch_faces} patch faces, {total_patch_tris} patch tris of {total_tris}"
    );
}
