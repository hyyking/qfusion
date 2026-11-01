use qf_engine::content::bsp::{
    Bsp, BspError, Format, TraceError, entities::parse_entities, read_pk3_entry,
};

fn le(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Builds a tiny legal FBSP: one planar quad (2 triangles) and one patch face.
fn synthetic_fbsp() -> Vec<u8> {
    let mut shader = vec![0u8; 72];
    shader[..8].copy_from_slice(b"textures");
    shader[64..68].copy_from_slice(&2i32.to_le_bytes());
    shader[68..72].copy_from_slice(&1i32.to_le_bytes());
    let mut model = Vec::new();
    for f in [0.0f32, 0.0, 0.0, 8.0, 8.0, 0.0] {
        model.extend(f.to_le_bytes());
    }
    for v in [0, 2, 0, 0] {
        model.extend(le(v));
    }
    let mut verts = Vec::new();
    for p in [
        [0.0f32, 0.0, 0.0],
        [8.0, 0.0, 0.0],
        [8.0, 8.0, 0.0],
        [0.0, 8.0, 0.0],
    ] {
        let mut v = vec![0u8; 80];
        for (i, c) in p.iter().enumerate() {
            v[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
        }
        v[52..56].copy_from_slice(&0f32.to_le_bytes());
        v[60..64].copy_from_slice(&1f32.to_le_bytes());
        verts.extend(v);
    }
    let elems: Vec<u8> = [0, 1, 2, 0, 2, 3].iter().flat_map(|&e| le(e)).collect();
    let mut planes = Vec::new();
    for (normal, dist) in [
        ([1.0f32, 0.0, 0.0], 1.0f32),
        ([-1.0, 0.0, 0.0], 1.0),
        ([0.0, 1.0, 0.0], 1.0),
        ([0.0, -1.0, 0.0], 1.0),
        ([0.0, 0.0, 1.0], 1.0),
        ([0.0, 0.0, -1.0], 1.0),
    ] {
        for component in normal {
            planes.extend(component.to_le_bytes());
        }
        planes.extend(dist.to_le_bytes());
    }
    let mut node = vec![0u8; 36];
    for (offset, value) in [(0, 0), (4, -1), (8, -2)] {
        node[offset..offset + 4].copy_from_slice(&le(value));
    }
    let mut leafs = Vec::new();
    for first_brush in [0, 1] {
        let mut leaf = vec![0u8; 48];
        for (offset, value) in [(40, first_brush), (44, 1)] {
            leaf[offset..offset + 4].copy_from_slice(&le(value));
        }
        leafs.extend(leaf);
    }
    let leaf_brushes: Vec<u8> = [0, 0].iter().flat_map(|&i| le(i)).collect();
    let mut brush = Vec::new();
    for value in [0, 6, 0] {
        brush.extend(le(value));
    }
    let mut brush_sides = Vec::new();
    for plane in 0..6 {
        brush_sides.extend(le(plane));
        brush_sides.extend(le(0));
        brush_sides.extend(le(-1));
    }
    let mut faces = Vec::new();
    for (ty, nv, ne) in [(1, 4, 6), (2, 0, 0)] {
        let mut f = vec![0u8; 148];
        for (o, v) in [(8, ty), (12, 0), (16, nv), (20, 0), (24, ne)] {
            f[o..o + 4].copy_from_slice(&le(v));
        }
        faces.extend(f);
    }
    let lumps: Vec<(usize, Vec<u8>)> = vec![
        (0, b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec()),
        (1, shader),
        (2, planes),
        (3, node),
        (4, leafs),
        (6, leaf_brushes),
        (8, brush),
        (9, brush_sides),
        (7, model),
        (10, verts),
        (11, elems),
        (13, faces),
        (15, vec![1, 2, 3, 4]),
    ];
    finish_bsp(b"FBSP", 1, 18, &lumps)
}

fn finish_bsp(
    magic: &[u8; 4],
    version: i32,
    lump_count: usize,
    lumps: &[(usize, Vec<u8>)],
) -> Vec<u8> {
    let header_len = 8 + lump_count * 8;
    let mut out = magic.to_vec();
    out.extend(le(version));
    let mut table = vec![(0i32, 0i32); lump_count];
    let mut body: Vec<u8> = Vec::new();
    for (i, data) in lumps {
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

fn synthetic_ibsp() -> Vec<u8> {
    let mut shader = vec![0u8; 72];
    shader[64..68].copy_from_slice(&2i32.to_le_bytes());
    shader[68..72].copy_from_slice(&1i32.to_le_bytes());
    let mut planes = Vec::new();
    for (normal, dist) in [
        ([1.0f32, 0.0, 0.0], 1.0f32),
        ([-1.0, 0.0, 0.0], 1.0),
        ([0.0, 1.0, 0.0], 1.0),
        ([0.0, -1.0, 0.0], 1.0),
        ([0.0, 0.0, 1.0], 1.0),
        ([0.0, 0.0, -1.0], 1.0),
    ] {
        for component in normal {
            planes.extend(component.to_le_bytes());
        }
        planes.extend(dist.to_le_bytes());
    }
    let mut node = vec![0u8; 36];
    for (offset, value) in [(0, 0), (4, -1), (8, -2)] {
        node[offset..offset + 4].copy_from_slice(&le(value));
    }
    let mut leafs = Vec::new();
    for first_brush in [0, 1] {
        let mut leaf = vec![0u8; 48];
        for (offset, value) in [(40, first_brush), (44, 1)] {
            leaf[offset..offset + 4].copy_from_slice(&le(value));
        }
        leafs.extend(leaf);
    }
    let mut brush_sides = Vec::new();
    for plane in 0..6 {
        brush_sides.extend(le(plane));
        brush_sides.extend(le(0));
    }
    let lumps = vec![
        (0, b"{\n\"classname\" \"worldspawn\"\n}\n\0".to_vec()),
        (1, shader),
        (2, planes),
        (3, node),
        (4, leafs),
        (6, vec![0, 0, 0, 0, 0, 0, 0, 0]),
        (8, [le(0), le(6), le(0)].concat()),
        (9, brush_sides),
    ];
    finish_bsp(b"IBSP", 46, 17, &lumps)
}

#[test]
fn parses_synthetic_map() {
    let bsp = Bsp::parse(&synthetic_fbsp()).unwrap();
    assert_eq!(bsp.format, Format::Fbsp);
    assert_eq!(
        (bsp.vertices.len(), bsp.faces.len(), bsp.elements.len()),
        (4, 2, 6)
    );
    assert!(bsp.entities.contains("worldspawn"));
    let mesh = bsp.model_mesh(0).unwrap();
    assert_eq!(
        (
            mesh.positions.len(),
            mesh.indices.len() / 3,
            mesh.skipped_faces
        ),
        (4, 2, 1)
    );
    assert_eq!(mesh.bounds().unwrap(), ([0.0, 0.0, 0.0], [8.0, 8.0, 0.0]));
    assert!(mesh.to_obj().contains("f 1 2 3"));
    let d = bsp.diagnostics.join("\n");
    assert!(d.contains("patch faces") && d.contains("lightgrid"));
}

#[test]
fn rejects_short_and_truncated_input() {
    assert!(matches!(Bsp::parse(&[]), Err(BspError::Truncated { .. })));
    let good = synthetic_fbsp();
    assert!(matches!(
        Bsp::parse(&good[..100]),
        Err(BspError::Truncated { .. })
    ));
    // every truncation point must fail cleanly (no panic)
    for n in (0..good.len()).step_by(7) {
        assert!(Bsp::parse(&good[..n]).is_err(), "len {n}");
    }
}

#[test]
fn rejects_bad_magic_and_version() {
    let mut d = synthetic_fbsp();
    d[0] = b'X';
    assert!(matches!(Bsp::parse(&d), Err(BspError::BadMagic(_))));
    let mut d = synthetic_fbsp();
    d[4] = 9;
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::UnsupportedVersion { .. })
    ));
    let mut d = synthetic_fbsp();
    d[0..4].copy_from_slice(b"RBSP");
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::UnsupportedLayout(_))
    ));
}

#[test]
fn rejects_bad_lumps_and_indices() {
    let mut d = synthetic_fbsp();
    d[8 + 8 * 10 + 4..8 + 8 * 10 + 8].copy_from_slice(&le(i32::MAX)); // vertexes len
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::LumpOutOfBounds { .. })
    ));
    let mut d = synthetic_fbsp();
    d[8 + 8 * 10 + 4..8 + 8 * 10 + 8].copy_from_slice(&le(81)); // not a multiple of 80
    assert!(matches!(Bsp::parse(&d), Err(BspError::BadLumpSize { .. })));
    let mut d = synthetic_fbsp();
    d[8 + 8 * 11..8 + 8 * 11 + 4].copy_from_slice(&le(-4)); // negative offset
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::LumpOutOfBounds { .. })
    ));
    let mut d = synthetic_fbsp();
    let off = i32::from_le_bytes(d[8 + 8 * 13..8 + 8 * 13 + 4].try_into().unwrap()) as usize;
    d[off + 24..off + 28].copy_from_slice(&le(5)); // numelems not a multiple of 3
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::IndexOutOfRange { .. })
    ));
    let mut d = synthetic_fbsp();
    let off = i32::from_le_bytes(d[8 + 8 * 13..8 + 8 * 13 + 4].try_into().unwrap()) as usize;
    d[off + 16..off + 20].copy_from_slice(&le(1000)); // face numverts
    assert!(matches!(
        Bsp::parse(&d),
        Err(BspError::IndexOutOfRange { .. })
    ));
}

#[test]
fn parses_and_validates_collision_structures() {
    let bsp = Bsp::parse(&synthetic_fbsp()).unwrap();
    assert_eq!(
        (bsp.nodes.len(), bsp.leafs.len(), bsp.leaf_brushes.len()),
        (1, 2, 2)
    );
    assert_eq!((bsp.brushes.len(), bsp.brush_sides.len()), (1, 6));
    assert_eq!(bsp.brushes[0].contents, 1);
    assert_eq!(bsp.brush_sides[0].surface, Some(-1));

    let mut bad = synthetic_fbsp();
    let side_offset = lump_offset(&bad, 9);
    bad[side_offset + 4..side_offset + 8].copy_from_slice(&le(5));
    assert!(matches!(
        Bsp::parse(&bad),
        Err(BspError::IndexOutOfRange { .. })
    ));

    let mut bad = synthetic_fbsp();
    let node_offset = lump_offset(&bad, 3);
    bad[node_offset + 4..node_offset + 8].copy_from_slice(&le(0));
    assert!(matches!(Bsp::parse(&bad), Err(BspError::NodeCycle { .. })));

    let mut bad = synthetic_fbsp();
    let leaf_brushes_offset = lump_offset(&bad, 6);
    bad[leaf_brushes_offset..leaf_brushes_offset + 4].copy_from_slice(&le(5));
    assert!(matches!(
        Bsp::parse(&bad),
        Err(BspError::IndexOutOfRange { .. })
    ));

    let mut bad = synthetic_fbsp();
    bad[8 + 3 * 8 + 4..8 + 3 * 8 + 8].copy_from_slice(&le(0));
    assert!(matches!(
        Bsp::parse(&bad),
        Err(BspError::MissingCollisionTree)
    ));
}

#[test]
fn supports_ibsp_collision_record_layout() {
    let bsp = Bsp::parse(&synthetic_ibsp()).unwrap();
    assert_eq!(bsp.format, Format::Ibsp);
    assert_eq!(bsp.brush_sides.len(), 6);
    assert!(bsp.brush_sides.iter().all(|side| side.surface.is_none()));
    let trace = bsp
        .trace([4.0, 0.0, 0.0], [-4.0, 0.0, 0.0], [0.0; 3], [0.0; 3], 1)
        .unwrap();
    assert!((trace.fraction - 0.37109375).abs() < 1e-5);
}

fn lump_offset(data: &[u8], lump: usize) -> usize {
    i32::from_le_bytes(data[8 + lump * 8..12 + lump * 8].try_into().unwrap()) as usize
}

#[test]
fn point_and_box_traces_match_legacy_brush_reference() {
    let bsp = Bsp::parse(&synthetic_fbsp()).unwrap();
    let point = bsp
        .trace([4.0, 0.0, 0.0], [-4.0, 0.0, 0.0], [0.0; 3], [0.0; 3], 1)
        .unwrap();
    assert!((point.fraction - 0.37109375).abs() < 1e-5);
    assert!((point.end_position[0] - 1.03125).abs() < 0.05);
    assert_eq!(point.plane.unwrap().normal, [1.0, 0.0, 0.0]);
    assert_eq!(point.contents, 1);
    assert_eq!(point.surface_flags, 2);

    let box_trace = bsp
        .trace([4.0, 0.0, 0.0], [-4.0, 0.0, 0.0], [-0.5; 3], [0.5; 3], 1)
        .unwrap();
    assert!((box_trace.fraction - 0.30859375).abs() < 1e-5);
    assert!((box_trace.end_position[0] - 1.53125).abs() < 0.05);

    let inside = bsp
        .trace([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], 1)
        .unwrap();
    assert!(inside.start_solid && inside.all_solid);
    assert_eq!(inside.fraction, 0.0);
    assert_eq!(
        bsp.trace([0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3], 0)
            .unwrap()
            .fraction,
        1.0
    );
}

#[test]
fn parses_entity_pairs_and_rejects_malformed_entity_lump() {
    let parsed =
        parse_entities("{ // world\n \"classname\" \"worldspawn\" \"message\" \"a\\\"b\" }")
            .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].get("classname"), Some("worldspawn"));
    assert_eq!(parsed[0].get("message"), Some("a\"b"));
    assert!(parse_entities("{ \"key\" \"value\" ").is_err());
    assert!(parse_entities("{ \"key\" \"value\" } trailing").is_err());

    let mut bad = synthetic_fbsp();
    let entities_offset = lump_offset(&bad, 0);
    let entities = b"{\n\"classname\" \"worldspawn\"\nx\n\0";
    bad[entities_offset..entities_offset + entities.len()].copy_from_slice(entities);
    assert!(matches!(
        Bsp::parse(&bad),
        Err(BspError::InvalidEntities(_))
    ));
}

#[test]
fn rejects_invalid_trace_inputs() {
    let bsp = Bsp::parse(&synthetic_fbsp()).unwrap();
    assert_eq!(
        bsp.trace([f32::NAN, 0.0, 0.0], [0.0; 3], [0.0; 3], [0.0; 3], 1),
        Err(TraceError::NonFiniteInput)
    );
    assert_eq!(
        bsp.trace([0.0; 3], [0.0; 3], [1.0, 0.0, 0.0], [0.0; 3], 1),
        Err(TraceError::InvertedBounds)
    );
}

#[test]
fn real_wbomb1_smoke() {
    let root = std::env::var("WARSOW_BASEWSW")
        .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
    let pk3 = std::path::Path::new(&root).join("map_wbomb1.pk3");
    if !pk3.exists() {
        eprintln!("SKIPPED: {} not found; real map NOT tested", pk3.display());
        return;
    }
    let data = read_pk3_entry(&pk3, "maps/wbomb1.bsp").unwrap();
    let bsp = Bsp::parse(&data).unwrap();
    assert_eq!((bsp.magic.as_str(), bsp.version), ("FBSP", 1));
    assert!(bsp.vertices.len() > 1000 && bsp.faces.len() > 1000);
    assert!(!bsp.nodes.is_empty() && !bsp.leafs.is_empty() && !bsp.brushes.is_empty());
    assert!(bsp.entities.contains("worldspawn"));
    let mesh = bsp.model_mesh(0).unwrap();
    assert!(mesh.indices.len() >= 3 && mesh.indices.chunks_exact(3).remainder().is_empty());
    assert!(
        mesh.indices
            .iter()
            .all(|&i| (i as usize) < mesh.positions.len())
    );
    assert!(mesh.positions.iter().flatten().all(|c| c.is_finite()));
}
