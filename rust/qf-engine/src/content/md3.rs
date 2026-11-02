//! MD3 model parser (`IDP3`, version 15). Headless: no GPU types.

use std::fmt;

const IDENT: &[u8; 4] = b"IDP3";
const VERSION: i32 = 15;
const HEADER_LEN: usize = 108;
const FRAME_LEN: usize = 56;
const TAG_LEN: usize = 112;
const SURFACE_HEADER_LEN: usize = 108;
const XYZ_SCALE: f32 = 1.0 / 64.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Md3Error {
    BadIdent,
    BadVersion(i32),
    Truncated(&'static str),
    BadCount(&'static str),
    BadIndex,
}

impl fmt::Display for Md3Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Md3Error::BadIdent => write!(f, "not an IDP3 model"),
            Md3Error::BadVersion(v) => write!(f, "unsupported md3 version {v}"),
            Md3Error::Truncated(w) => write!(f, "md3 truncated while reading {w}"),
            Md3Error::BadCount(w) => write!(f, "md3 has an invalid {w} count"),
            Md3Error::BadIndex => write!(f, "md3 triangle index out of range"),
        }
    }
}

impl std::error::Error for Md3Error {}

#[derive(Debug, Clone, PartialEq)]
pub struct Md3Frame {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub origin: [f32; 3],
    pub radius: f32,
    pub name: String,
}

/// Attachment point; `axis` rows are the forward, left and up vectors.
#[derive(Debug, Clone, PartialEq)]
pub struct Md3Tag {
    pub name: String,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Md3Surface {
    pub name: String,
    pub shaders: Vec<String>,
    pub triangles: Vec<[u32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// `frames * verts` positions, frame-major.
    pub positions: Vec<[f32; 3]>,
    /// Same layout as `positions`.
    pub normals: Vec<[f32; 3]>,
    pub num_verts: usize,
}

impl Md3Surface {
    pub fn frame_positions(&self, frame: usize) -> &[[f32; 3]] {
        &self.positions[frame * self.num_verts..(frame + 1) * self.num_verts]
    }

    pub fn frame_normals(&self, frame: usize) -> &[[f32; 3]] {
        &self.normals[frame * self.num_verts..(frame + 1) * self.num_verts]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Md3Model {
    pub name: String,
    pub frames: Vec<Md3Frame>,
    /// `frames * tags_per_frame`, frame-major.
    pub tags: Vec<Md3Tag>,
    pub num_tags: usize,
    pub surfaces: Vec<Md3Surface>,
}

impl Md3Model {
    pub fn frame_tags(&self, frame: usize) -> &[Md3Tag] {
        &self.tags[frame * self.num_tags..(frame + 1) * self.num_tags]
    }

    pub fn tag(&self, frame: usize, name: &str) -> Option<&Md3Tag> {
        self.frame_tags(frame).iter().find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn slice(&self, ofs: usize, len: usize, what: &'static str) -> Result<&[u8], Md3Error> {
        ofs.checked_add(len)
            .and_then(|end| self.0.get(ofs..end))
            .ok_or(Md3Error::Truncated(what))
    }

    fn i32(&self, ofs: usize, what: &'static str) -> Result<i32, Md3Error> {
        let b = self.slice(ofs, 4, what)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn count(&self, ofs: usize, what: &'static str) -> Result<usize, Md3Error> {
        usize::try_from(self.i32(ofs, what)?).map_err(|_| Md3Error::BadCount(what))
    }

    fn f32(&self, ofs: usize, what: &'static str) -> Result<f32, Md3Error> {
        Ok(f32::from_bits(self.i32(ofs, what)? as u32))
    }

    fn vec3(&self, ofs: usize, what: &'static str) -> Result<[f32; 3], Md3Error> {
        Ok([self.f32(ofs, what)?, self.f32(ofs + 4, what)?, self.f32(ofs + 8, what)?])
    }

    fn string(&self, ofs: usize, len: usize, what: &'static str) -> Result<String, Md3Error> {
        let b = self.slice(ofs, len, what)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
}

fn decode_normal(packed: u16) -> [f32; 3] {
    let scale = std::f32::consts::TAU / 255.0;
    let lat = f32::from((packed >> 8) as u8) * scale;
    let lng = f32::from((packed & 0xff) as u8) * scale;
    [lat.cos() * lng.sin(), lat.sin() * lng.sin(), lng.cos()]
}

pub fn parse(data: &[u8]) -> Result<Md3Model, Md3Error> {
    let r = Reader(data);
    if r.slice(0, 4, "header")? != IDENT {
        return Err(Md3Error::BadIdent);
    }
    let version = r.i32(4, "header")?;
    if version != VERSION {
        return Err(Md3Error::BadVersion(version));
    }
    r.slice(0, HEADER_LEN, "header")?;
    let name = r.string(8, 64, "name")?;
    let num_frames = r.count(76, "frame")?;
    let num_tags = r.count(80, "tag")?;
    let num_surfaces = r.count(84, "surface")?;
    let ofs_frames = r.count(92, "frame offset")?;
    let ofs_tags = r.count(96, "tag offset")?;
    let ofs_surfaces = r.count(100, "surface offset")?;
    if num_frames == 0 {
        return Err(Md3Error::BadCount("frame"));
    }

    let mut frames = Vec::with_capacity(num_frames.min(4096));
    for i in 0..num_frames {
        let o = ofs_frames + i * FRAME_LEN;
        frames.push(Md3Frame {
            mins: r.vec3(o, "frame")?,
            maxs: r.vec3(o + 12, "frame")?,
            origin: r.vec3(o + 24, "frame")?,
            radius: r.f32(o + 36, "frame")?,
            name: r.string(o + 40, 16, "frame")?,
        });
    }

    let mut tags = Vec::new();
    for i in 0..num_frames * num_tags {
        let o = ofs_tags + i * TAG_LEN;
        let mut axis = [[0.0; 3]; 3];
        for (k, row) in axis.iter_mut().enumerate() {
            *row = r.vec3(o + 76 + k * 12, "tag")?;
        }
        tags.push(Md3Tag { name: r.string(o, 64, "tag")?, origin: r.vec3(o + 64, "tag")?, axis });
    }

    let mut surfaces = Vec::with_capacity(num_surfaces.min(256));
    let mut so = ofs_surfaces;
    for _ in 0..num_surfaces {
        r.slice(so, SURFACE_HEADER_LEN, "surface")?;
        let sframes = r.count(so + 72, "surface frame")?;
        let num_shaders = r.count(so + 76, "shader")?;
        let num_verts = r.count(so + 80, "vertex")?;
        let num_tris = r.count(so + 84, "triangle")?;
        let ofs_tris = r.count(so + 88, "triangle offset")?;
        let ofs_shaders = r.count(so + 92, "shader offset")?;
        let ofs_st = r.count(so + 96, "st offset")?;
        let ofs_xyz = r.count(so + 100, "xyz offset")?;
        let ofs_end = r.count(so + 104, "surface end")?;

        let mut shaders = Vec::new();
        for i in 0..num_shaders {
            shaders.push(r.string(so + ofs_shaders + i * 68, 64, "shader")?);
        }
        let mut triangles = Vec::new();
        for i in 0..num_tris {
            let o = so + ofs_tris + i * 12;
            let mut t = [0u32; 3];
            for (k, v) in t.iter_mut().enumerate() {
                *v = u32::try_from(r.i32(o + k * 4, "triangle")?).map_err(|_| Md3Error::BadIndex)?;
                if *v as usize >= num_verts {
                    return Err(Md3Error::BadIndex);
                }
            }
            triangles.push(t);
        }
        let mut uvs = Vec::new();
        for i in 0..num_verts {
            let o = so + ofs_st + i * 8;
            uvs.push([r.f32(o, "st")?, r.f32(o + 4, "st")?]);
        }
        let total = sframes * num_verts;
        let xyz = r.slice(so + ofs_xyz, total * 8, "vertices")?;
        let mut positions = Vec::with_capacity(total);
        let mut normals = Vec::with_capacity(total);
        for v in xyz.chunks_exact(8) {
            let s = |i: usize| f32::from(i16::from_le_bytes([v[i], v[i + 1]])) * XYZ_SCALE;
            positions.push([s(0), s(2), s(4)]);
            normals.push(decode_normal(u16::from_le_bytes([v[6], v[7]])));
        }
        surfaces.push(Md3Surface {
            name: r.string(so + 4, 64, "surface name")?,
            shaders,
            triangles,
            uvs,
            positions,
            normals,
            num_verts,
        });
        if ofs_end == 0 {
            return Err(Md3Error::BadCount("surface end"));
        }
        so += ofs_end;
    }

    Ok(Md3Model { name, frames, tags, num_tags, surfaces })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(buf: &mut Vec<u8>, v: i32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn fixed(buf: &mut Vec<u8>, s: &str, len: usize) {
        let mut b = s.as_bytes().to_vec();
        b.resize(len, 0);
        buf.extend(b);
    }

    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(IDENT);
        put(&mut b, VERSION);
        fixed(&mut b, "test", 64);
        put(&mut b, 0);
        put(&mut b, 1); // frames
        put(&mut b, 1); // tags
        put(&mut b, 1); // surfaces
        put(&mut b, 0);
        put(&mut b, 108); // frames
        put(&mut b, 108 + 56); // tags
        put(&mut b, 108 + 56 + 112); // surfaces
        put(&mut b, 0);
        for _ in 0..10 {
            put(&mut b, 0);
        }
        fixed(&mut b, "f0", 16);
        fixed(&mut b, "tag_x", 64);
        for v in [1.0f32, 2.0, 3.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            put(&mut b, v.to_bits() as i32);
        }
        let surf_start = b.len();
        b.extend_from_slice(IDENT);
        fixed(&mut b, "surf", 64);
        put(&mut b, 0);
        put(&mut b, 1);
        put(&mut b, 1); // shaders
        put(&mut b, 3); // verts
        put(&mut b, 1); // tris
        put(&mut b, 108); // tris
        put(&mut b, 108 + 12); // shaders
        put(&mut b, 108 + 12 + 68); // st
        put(&mut b, 108 + 12 + 68 + 24); // xyz
        put(&mut b, 108 + 12 + 68 + 24 + 24); // end
        for i in 0..3 {
            put(&mut b, i);
        }
        fixed(&mut b, "textures/x", 64);
        put(&mut b, 0);
        for _ in 0..6 {
            put(&mut b, 0x3f00_0000);
        }
        for i in 0..3i16 {
            for c in [64 * i, 0, 128, 0] {
                b.extend_from_slice(&c.to_le_bytes());
            }
        }
        assert_eq!(b.len() - surf_start, 108 + 12 + 68 + 24 + 24);
        b
    }

    #[test]
    fn parses_synthetic_model() {
        let m = parse(&sample()).unwrap();
        assert_eq!(m.name, "test");
        assert_eq!(m.frames.len(), 1);
        assert_eq!(m.tag(0, "TAG_X").unwrap().origin, [1.0, 2.0, 3.0]);
        let s = &m.surfaces[0];
        assert_eq!(s.shaders, ["textures/x"]);
        assert_eq!(s.triangles, [[0, 1, 2]]);
        assert_eq!(s.frame_positions(0)[1], [1.0, 0.0, 2.0]);
        assert_eq!(s.uvs[0], [0.5, 0.5]);
        let n = s.frame_normals(0)[0];
        assert!((n[0] * n[0] + n[1] * n[1] + n[2] * n[2] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(parse(b"XXXX"), Err(Md3Error::BadIdent));
        let mut d = sample();
        d[0] = b'X';
        assert_eq!(parse(&d), Err(Md3Error::BadIdent));
        let d = sample();
        assert!(parse(&d[..d.len() - 10]).is_err());
    }
}
