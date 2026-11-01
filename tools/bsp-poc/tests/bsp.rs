use bsp_poc::{read_pk3_entry, Bsp, BspError, Format};

fn le(v: i32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Builds a tiny legal FBSP: one planar quad (2 triangles) and one patch face.
fn synthetic_fbsp() -> Vec<u8> {
    let mut shader = vec![0u8; 72];
    shader[..8].copy_from_slice(b"textures");
    let mut model = Vec::new();
    for f in [0.0f32, 0.0, 0.0, 8.0, 8.0, 0.0] {
        model.extend(f.to_le_bytes());
    }
    for v in [0, 2, 0, 0] {
        model.extend(le(v));
    }
    let mut verts = Vec::new();
    for p in [[0.0f32, 0.0, 0.0], [8.0, 0.0, 0.0], [8.0, 8.0, 0.0], [0.0, 8.0, 0.0]] {
        let mut v = vec![0u8; 80];
        for (i, c) in p.iter().enumerate() {
            v[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
        }
        v[52..56].copy_from_slice(&0f32.to_le_bytes());
        v[60..64].copy_from_slice(&1f32.to_le_bytes());
        verts.extend(v);
    }
    let elems: Vec<u8> = [0, 1, 2, 0, 2, 3].iter().flat_map(|&e| le(e)).collect();
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
        (7, model),
        (10, verts),
        (11, elems),
        (13, faces),
        (15, vec![1, 2, 3, 4]),
    ];
    let mut out = b"FBSP".to_vec();
    out.extend(le(1));
    let mut table = vec![(0i32, 0i32); 18];
    let mut body: Vec<u8> = Vec::new();
    for (i, d) in &lumps {
        table[*i] = ((152 + body.len()) as i32, d.len() as i32);
        body.extend(d);
    }
    for (o, l) in table {
        out.extend(le(o));
        out.extend(le(l));
    }
    out.extend(body);
    out
}

#[test]
fn parses_synthetic_map() {
    let bsp = Bsp::parse(&synthetic_fbsp()).unwrap();
    assert_eq!(bsp.format, Format::Fbsp);
    assert_eq!((bsp.vertices.len(), bsp.faces.len(), bsp.elements.len()), (4, 2, 6));
    assert!(bsp.entities.contains("worldspawn"));
    let mesh = bsp.model_mesh(0).unwrap();
    assert_eq!((mesh.positions.len(), mesh.indices.len() / 3, mesh.skipped_faces), (4, 2, 1));
    assert_eq!(mesh.bounds().unwrap(), ([0.0, 0.0, 0.0], [8.0, 8.0, 0.0]));
    assert!(mesh.to_obj().contains("f 1 2 3"));
    let d = bsp.diagnostics.join("\n");
    assert!(d.contains("patch faces") && d.contains("lightgrid"));
}

#[test]
fn rejects_short_and_truncated_input() {
    assert!(matches!(Bsp::parse(&[]), Err(BspError::Truncated { .. })));
    let good = synthetic_fbsp();
    assert!(matches!(Bsp::parse(&good[..100]), Err(BspError::Truncated { .. })));
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
    assert!(matches!(Bsp::parse(&d), Err(BspError::UnsupportedVersion { .. })));
    let mut d = synthetic_fbsp();
    d[0..4].copy_from_slice(b"RBSP");
    assert!(matches!(Bsp::parse(&d), Err(BspError::UnsupportedLayout(_))));
}

#[test]
fn rejects_bad_lumps_and_indices() {
    let mut d = synthetic_fbsp();
    d[8 + 8 * 10 + 4..8 + 8 * 10 + 8].copy_from_slice(&le(i32::MAX)); // vertexes len
    assert!(matches!(Bsp::parse(&d), Err(BspError::LumpOutOfBounds { .. })));
    let mut d = synthetic_fbsp();
    d[8 + 8 * 10 + 4..8 + 8 * 10 + 8].copy_from_slice(&le(81)); // not a multiple of 80
    assert!(matches!(Bsp::parse(&d), Err(BspError::BadLumpSize { .. })));
    let mut d = synthetic_fbsp();
    d[8 + 8 * 11..8 + 8 * 11 + 4].copy_from_slice(&le(-4)); // negative offset
    assert!(matches!(Bsp::parse(&d), Err(BspError::LumpOutOfBounds { .. })));
    let mut d = synthetic_fbsp();
    let off = i32::from_le_bytes(d[8 + 8 * 13..8 + 8 * 13 + 4].try_into().unwrap()) as usize;
    d[off + 16..off + 20].copy_from_slice(&le(1000)); // face numverts
    assert!(matches!(Bsp::parse(&d), Err(BspError::IndexOutOfRange { .. })));
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
    assert!(bsp.entities.contains("worldspawn"));
    let mesh = bsp.model_mesh(0).unwrap();
    assert!(mesh.indices.len() >= 3 && mesh.indices.len() % 3 == 0);
    assert!(mesh.indices.iter().all(|&i| (i as usize) < mesh.positions.len()));
    assert!(mesh.positions.iter().flatten().all(|c| c.is_finite()));
}
