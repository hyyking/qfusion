//! Bounded BSP reader for Warsow/qfusion maps.
//!
//! Layouts follow `source/common/facilities/qfiles.h`. Supported: `FBSP` v1
//! (qfusion, 18 lumps; what `wbomb1` uses) and `IBSP` v46/47 (17 lumps).
//! `RBSP` is recognised but its layout is not decoded.

use std::fmt;

pub const LUMP_NAMES: [&str; 18] = [
    "entities", "shaderrefs", "planes", "nodes", "leafs", "leaffaces", "leafbrushes", "models",
    "brushes", "brushsides", "vertexes", "elements", "fogs", "faces", "lighting", "lightgrid",
    "visibility", "lightarray",
];
const L_ENTITIES: usize = 0;
const L_SHADERS: usize = 1;
const L_PLANES: usize = 2;
const L_NODES: usize = 3;
const L_LEAFS: usize = 4;
const L_MODELS: usize = 7;
const L_BRUSHES: usize = 8;
const L_BRUSHSIDES: usize = 9;
const L_VERTEXES: usize = 10;
const L_ELEMENTS: usize = 11;
const L_FOGS: usize = 12;
const L_FACES: usize = 13;

pub const FACETYPE_PLANAR: i32 = 1;
pub const FACETYPE_PATCH: i32 = 2;
pub const FACETYPE_TRISURF: i32 = 3;
pub const FACETYPE_FLARE: i32 = 4;
pub const FACETYPE_FOLIAGE: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BspError {
    Truncated { needed: usize, actual: usize },
    BadMagic([u8; 4]),
    UnsupportedVersion { magic: String, version: i32 },
    UnsupportedLayout(String),
    LumpOutOfBounds { lump: &'static str, offset: i64, len: i64, file_len: usize },
    BadLumpSize { lump: &'static str, len: usize, item_size: usize },
    IndexOutOfRange { what: String, index: i64, limit: usize },
}

impl fmt::Display for BspError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BspError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// "IBSP" (Quake 3 / RTCW), 17 lumps.
    Ibsp,
    /// "FBSP" (qfusion), 18 lumps, 4 lightmap styles per face/vertex.
    Fbsp,
}

#[derive(Debug, Clone, Copy)]
pub struct Lump {
    pub offset: usize,
    pub len: usize,
}

#[derive(Debug, Clone)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Face {
    pub shader: i32,
    pub fog: i32,
    pub facetype: i32,
    pub first_vert: i32,
    pub num_verts: i32,
    pub first_elem: i32,
    pub num_elems: i32,
    pub patch_cp: [i32; 2],
}

#[derive(Debug, Clone)]
pub struct Model {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub first_face: i32,
    pub num_faces: i32,
    pub first_brush: i32,
    pub num_brushes: i32,
}

#[derive(Debug, Clone)]
pub struct ShaderRef {
    pub name: String,
    pub surface_flags: i32,
    pub contents: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct Plane {
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug, Clone)]
pub struct Bsp {
    pub format: Format,
    pub magic: String,
    pub version: i32,
    pub lumps: Vec<Lump>,
    pub entities: String,
    pub shaders: Vec<ShaderRef>,
    pub planes: Vec<Plane>,
    pub models: Vec<Model>,
    pub vertices: Vec<Vertex>,
    pub elements: Vec<u32>,
    pub faces: Vec<Face>,
    /// Record counts of lumps that are size-checked but not otherwise decoded.
    pub counts: Counts,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub nodes: usize,
    pub leafs: usize,
    pub brushes: usize,
    pub brushsides: usize,
    pub fogs: usize,
}

struct Rd<'a>(&'a [u8]);
impl Rd<'_> {
    fn i32(&self, o: usize) -> i32 {
        i32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn f32(&self, o: usize) -> f32 {
        f32::from_le_bytes(self.0[o..o + 4].try_into().unwrap())
    }
    fn f3(&self, o: usize) -> [f32; 3] {
        [self.f32(o), self.f32(o + 4), self.f32(o + 8)]
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

impl Bsp {
    pub fn parse(data: &[u8]) -> Result<Bsp, BspError> {
        if data.len() < 8 {
            return Err(BspError::Truncated { needed: 8, actual: data.len() });
        }
        let magic: [u8; 4] = data[0..4].try_into().unwrap();
        let version = i32::from_le_bytes(data[4..8].try_into().unwrap());
        let magic_s = String::from_utf8_lossy(&magic).into_owned();
        let (format, nlumps) = match (&magic, version) {
            (b"FBSP", 1) => (Format::Fbsp, 18),
            (b"IBSP", 46 | 47) => (Format::Ibsp, 17),
            (b"RBSP", _) => return Err(BspError::UnsupportedLayout("RBSP layout is not decoded".into())),
            (b"FBSP" | b"IBSP", v) => {
                return Err(BspError::UnsupportedVersion { magic: magic_s, version: v })
            }
            _ => return Err(BspError::BadMagic(magic)),
        };
        let header_len = 8 + nlumps * 8;
        if data.len() < header_len {
            return Err(BspError::Truncated { needed: header_len, actual: data.len() });
        }
        let h = Rd(data);
        let mut lumps = Vec::new();
        for i in 0..nlumps {
            let (o, l) = (h.i32(8 + i * 8) as i64, h.i32(12 + i * 8) as i64);
            if o < 0 || l < 0 || o + l > data.len() as i64 {
                return Err(BspError::LumpOutOfBounds { lump: LUMP_NAMES[i], offset: o, len: l, file_len: data.len() });
            }
            lumps.push(Lump { offset: o as usize, len: l as usize });
        }
        let slice = |i: usize| &data[lumps[i].offset..lumps[i].offset + lumps[i].len];
        let sized = |i: usize, size: usize| -> Result<usize, BspError> {
            if lumps[i].len % size != 0 {
                Err(BspError::BadLumpSize { lump: LUMP_NAMES[i], len: lumps[i].len, item_size: size })
            } else {
                Ok(lumps[i].len / size)
            }
        };
        // (vertex size, normal offset, face size, brushside size)
        let (vsize, voff_n, fsize, bssize) = match format {
            Format::Fbsp => (80, 52, 148, 12),
            Format::Ibsp => (44, 28, 104, 8),
        };

        let entities = cstr(slice(L_ENTITIES));
        let n = sized(L_SHADERS, 72)?;
        let r = Rd(slice(L_SHADERS));
        let shaders = (0..n)
            .map(|i| ShaderRef {
                name: cstr(&r.0[i * 72..i * 72 + 64]),
                surface_flags: r.i32(i * 72 + 64),
                contents: r.i32(i * 72 + 68),
            })
            .collect();
        let n = sized(L_PLANES, 16)?;
        let r = Rd(slice(L_PLANES));
        let planes = (0..n).map(|i| Plane { normal: r.f3(i * 16), dist: r.f32(i * 16 + 12) }).collect();
        let n = sized(L_MODELS, 40)?;
        let r = Rd(slice(L_MODELS));
        let models = (0..n)
            .map(|i| {
                let o = i * 40;
                Model {
                    mins: r.f3(o),
                    maxs: r.f3(o + 12),
                    first_face: r.i32(o + 24),
                    num_faces: r.i32(o + 28),
                    first_brush: r.i32(o + 32),
                    num_brushes: r.i32(o + 36),
                }
            })
            .collect();
        let n = sized(L_VERTEXES, vsize)?;
        let r = Rd(slice(L_VERTEXES));
        let vertices = (0..n)
            .map(|i| {
                let o = i * vsize;
                Vertex { pos: r.f3(o), uv: [r.f32(o + 12), r.f32(o + 16)], normal: r.f3(o + voff_n) }
            })
            .collect::<Vec<_>>();
        let n = sized(L_ELEMENTS, 4)?;
        let r = Rd(slice(L_ELEMENTS));
        let elements = (0..n).map(|i| r.i32(i * 4) as u32).collect();
        let n = sized(L_FACES, fsize)?;
        let r = Rd(slice(L_FACES));
        let pc = fsize - 8;
        let faces = (0..n)
            .map(|i| {
                let o = i * fsize;
                Face {
                    shader: r.i32(o),
                    fog: r.i32(o + 4),
                    facetype: r.i32(o + 8),
                    first_vert: r.i32(o + 12),
                    num_verts: r.i32(o + 16),
                    first_elem: r.i32(o + 20),
                    num_elems: r.i32(o + 24),
                    patch_cp: [r.i32(o + pc), r.i32(o + pc + 4)],
                }
            })
            .collect();
        let counts = Counts {
            nodes: sized(L_NODES, 36)?,
            leafs: sized(L_LEAFS, 48)?,
            brushes: sized(L_BRUSHES, 12)?,
            brushsides: sized(L_BRUSHSIDES, bssize)?,
            fogs: sized(L_FOGS, 72)?,
        };

        let mut bsp = Bsp {
            format, magic: magic_s, version, lumps, entities, shaders, planes, models,
            vertices, elements, faces, counts, diagnostics: Vec::new(),
        };
        bsp.validate()?;
        for i in 14..nlumps {
            if bsp.lumps[i].len > 0 {
                bsp.diagnostics.push(format!(
                    "lump {} ({}) present ({} bytes): preserved as offset/len only, not decoded",
                    i, LUMP_NAMES[i], bsp.lumps[i].len
                ));
            }
        }
        for (name, c) in [
            ("nodes", bsp.counts.nodes), ("leafs", bsp.counts.leafs), ("brushes", bsp.counts.brushes),
            ("brushsides", bsp.counts.brushsides), ("fogs", bsp.counts.fogs),
        ] {
            bsp.diagnostics.push(format!("lump {name}: {c} records size-checked, contents not decoded (collision/vis data)"));
        }
        bsp.diagnostics.push("leaffaces/leafbrushes lumps are not inspected".into());
        let mut by_type = [0usize; 6];
        for f in &bsp.faces {
            by_type[f.facetype.clamp(0, 5) as usize] += 1;
        }
        if by_type[FACETYPE_PATCH as usize] > 0 {
            bsp.diagnostics.push(format!("{} patch faces (bezier) are not tessellated", by_type[2]));
        }
        for (t, n) in [(FACETYPE_FLARE, "flare"), (FACETYPE_FOLIAGE, "foliage"), (0, "bad-type")] {
            if by_type[t as usize] > 0 {
                bsp.diagnostics.push(format!("{} {n} faces are skipped", by_type[t as usize]));
            }
        }
        Ok(bsp)
    }

    fn validate(&self) -> Result<(), BspError> {
        for f in &self.faces {
            if f.shader < 0 || f.shader as usize >= self.shaders.len() {
                return Err(BspError::IndexOutOfRange {
                    what: "face.shader".into(), index: f.shader as i64, limit: self.shaders.len(),
                });
            }
            if matches!(f.facetype, FACETYPE_PLANAR | FACETYPE_TRISURF | FACETYPE_PATCH) {
                check_range("face verts", f.first_vert, f.num_verts, self.vertices.len())?;
                check_range("face elements", f.first_elem, f.num_elems, self.elements.len())?;
            }
            if matches!(f.facetype, FACETYPE_PLANAR | FACETYPE_TRISURF) && f.num_elems % 3 != 0 {
                return Err(BspError::IndexOutOfRange {
                    what: "face elements (not a multiple of 3)".into(), index: f.num_elems as i64, limit: 3,
                });
            }
            if matches!(f.facetype, FACETYPE_PLANAR | FACETYPE_TRISURF) {
                let s = f.first_elem as usize;
                for &e in &self.elements[s..s + f.num_elems as usize] {
                    if e >= f.num_verts as u32 {
                        return Err(BspError::IndexOutOfRange {
                            what: "element".into(), index: e as i64, limit: f.num_verts as usize,
                        });
                    }
                }
            }
        }
        for m in &self.models {
            check_range("model faces", m.first_face, m.num_faces, self.faces.len())?;
        }
        Ok(())
    }

    /// Triangle mesh of a model (0 = world): planar and trisurf faces only.
    pub fn model_mesh(&self, model: usize) -> Result<WorldMesh, BspError> {
        let m = self.models.get(model).ok_or(BspError::IndexOutOfRange {
            what: "model".into(), index: model as i64, limit: self.models.len(),
        })?;
        let mut mesh = WorldMesh::default();
        for f in &self.faces[m.first_face as usize..(m.first_face + m.num_faces) as usize] {
            if !matches!(f.facetype, FACETYPE_PLANAR | FACETYPE_TRISURF) {
                mesh.skipped_faces += 1;
                continue;
            }
            let base = mesh.positions.len() as u32;
            for v in &self.vertices[f.first_vert as usize..(f.first_vert + f.num_verts) as usize] {
                mesh.positions.push(v.pos);
                mesh.normals.push(v.normal);
                mesh.uvs.push(v.uv);
            }
            let e = &self.elements[f.first_elem as usize..(f.first_elem + f.num_elems) as usize];
            for tri in e.chunks_exact(3) {
                mesh.indices.extend(tri.iter().map(|i| base + i));
                mesh.triangle_shaders.push(f.shader as u32);
            }
        }
        Ok(mesh)
    }
}

fn check_range(what: &str, first: i32, num: i32, limit: usize) -> Result<(), BspError> {
    if first < 0 || num < 0 || (first as i64 + num as i64) > limit as i64 {
        return Err(BspError::IndexOutOfRange { what: what.into(), index: first as i64 + num as i64, limit });
    }
    Ok(())
}

#[derive(Debug, Default, Clone)]
pub struct WorldMesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Shader reference index per triangle.
    pub triangle_shaders: Vec<u32>,
    pub skipped_faces: usize,
}

impl WorldMesh {
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.positions.iter();
        let first = *it.next()?;
        Some(it.fold((first, first), |(mut lo, mut hi), p| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            (lo, hi)
        }))
    }

    pub fn to_obj(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        for p in &self.positions {
            let _ = writeln!(s, "v {} {} {}", p[0], p[1], p[2]);
        }
        for t in self.indices.chunks_exact(3) {
            let _ = writeln!(s, "f {} {} {}", t[0] + 1, t[1] + 1, t[2] + 1);
        }
        s
    }
}

/// Reads one entry (e.g. `maps/wbomb1.bsp`) out of a pk3 (zip) archive in memory.
pub fn read_pk3_entry(pk3: &std::path::Path, entry: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let file = std::fs::File::open(pk3).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut f = z.by_name(entry).map_err(|e| format!("{entry}: {e}"))?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}
