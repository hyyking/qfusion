//! GPU-independent image decoding (tga/jpg/png) and mip chain generation.

use std::fmt;

use super::vfs::Vfs;

/// Extensions tried, in legacy order, when a texture name has none or a missing one.
const EXTENSIONS: [&str; 4] = ["tga", "jpg", "png", "ktx"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8Image {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA, row-major, top row first.
    pub pixels: Vec<u8>,
}

#[derive(Debug)]
pub enum ImageError {
    NotFound(String),
    Decode(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(n) => write!(f, "image `{n}` not found"),
            Self::Decode(e) => write!(f, "image decode failed: {e}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl Rgba8Image {
    pub fn decode(data: &[u8]) -> Result<Self, ImageError> {
        Self::decode_hint(data, None)
    }

    /// Decodes with the format taken from a file extension when known; TGA has no magic
    /// number, so content sniffing alone cannot identify it.
    pub fn decode_hint(data: &[u8], ext: Option<&str>) -> Result<Self, ImageError> {
        if data.starts_with(&KTX_MAGIC) || ext.is_some_and(|e| e.eq_ignore_ascii_case("ktx")) {
            return Self::decode_ktx(data);
        }
        let format = ext.and_then(image::ImageFormat::from_extension);
        let loaded = match format {
            Some(f) => image::load_from_memory_with_format(data, f),
            None => image::load_from_memory(data),
        };
        let img = match loaded {
            Ok(img) => img,
            Err(e) => {
                // Some stock TGAs carry an alpha descriptor on 24-bit data, which the `image`
                // crate rejects; the legacy loader ignores it.
                if format == Some(image::ImageFormat::Tga)
                    && let Ok(tga) = decode_tga(data)
                {
                    return Ok(tga);
                }
                return Err(ImageError::Decode(e.to_string()));
            }
        };
        let rgba = img.to_rgba8();
        Ok(Self {
            width: rgba.width(),
            height: rgba.height(),
            pixels: rgba.into_raw(),
        })
    }

    /// KTX1 container: uncompressed 8-bit formats (luminance, alpha, RGB(A), BGR(A)) and
    /// block-compressed ones (ETC1/2, BC1-3 via `texture2ddecoder`). Returns the first mip.
    fn decode_ktx(data: &[u8]) -> Result<Self, ImageError> {
        Ok(decode_ktx_levels(data)?.swap_remove(0))
    }

    /// Solid-colour checkerboard used where an asset is missing.
    pub fn checker(size: u32, cell: u32) -> Self {
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let on = ((x / cell) + (y / cell)).is_multiple_of(2);
                pixels.extend_from_slice(if on {
                    &[255, 0, 255, 255]
                } else {
                    &[20, 20, 20, 255]
                });
            }
        }
        Self {
            width: size,
            height: size,
            pixels,
        }
    }

    pub fn solid(rgba: [u8; 4]) -> Self {
        Self {
            width: 1,
            height: 1,
            pixels: rgba.to_vec(),
        }
    }

    /// Full mip chain down to 1x1, starting with `self`; each level is a 2x2 box filter.
    pub fn mip_chain(&self) -> Vec<Rgba8Image> {
        let mut chain = vec![self.clone()];
        while let Some(last) = chain.last() {
            if last.width == 1 && last.height == 1 {
                break;
            }
            let next = last.half();
            chain.push(next);
        }
        chain
    }

    fn half(&self) -> Self {
        let (w, h) = ((self.width / 2).max(1), (self.height / 2).max(1));
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let xs = [(x * 2).min(self.width - 1), (x * 2 + 1).min(self.width - 1)];
                let ys = [
                    (y * 2).min(self.height - 1),
                    (y * 2 + 1).min(self.height - 1),
                ];
                for c in 0..4 {
                    let mut sum = 0u32;
                    for &sy in &ys {
                        for &sx in &xs {
                            sum += self.pixels[((sy * self.width + sx) * 4) as usize + c] as u32;
                        }
                    }
                    pixels.push(((sum + 2) / 4) as u8);
                }
            }
        }
        Self {
            width: w,
            height: h,
            pixels,
        }
    }

    pub fn has_alpha(&self) -> bool {
        self.pixels.chunks_exact(4).any(|p| p[3] != 255)
    }
}

/// Resolves a shader-style texture name (with or without extension) to a VFS path.
pub fn find_image(vfs: &Vfs, name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let stem = match name.rsplit_once('.') {
        Some((stem, ext)) if EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)) => stem,
        _ => name.as_str(),
    };
    // An exact hit on the given extension wins, then the legacy tga/jpg/png order.
    if name != stem && vfs.exists(&name) {
        return Some(name);
    }
    EXTENSIONS
        .iter()
        .map(|ext| format!("{stem}.{ext}"))
        .find(|candidate| vfs.exists(candidate))
}

/// Engine-provided images referenced by shaders: `$whiteimage`/`*white`, `$blackimage`/`*black`
/// and `$greyimage`/`*grey`.
pub fn builtin_image(name: &str) -> Option<Rgba8Image> {
    let rgb = match name.to_ascii_lowercase().as_str() {
        "$whiteimage" | "*white" => 255,
        "$blackimage" | "*black" => 0,
        "$greyimage" | "*grey" | "$grayimage" | "*gray" => 128,
        _ => return None,
    };
    Some(Rgba8Image::solid([rgb, rgb, rgb, 255]))
}

pub fn load_image(vfs: &Vfs, name: &str) -> Result<Rgba8Image, ImageError> {
    if let Some(img) = builtin_image(name) {
        return Ok(img);
    }
    let data = read_image(vfs, name)?;
    Rgba8Image::decode_hint(&data.1, data.0.rsplit_once('.').map(|(_, e)| e))
}

fn read_image(vfs: &Vfs, name: &str) -> Result<(String, Vec<u8>), ImageError> {
    let path = find_image(vfs, name).ok_or_else(|| ImageError::NotFound(name.to_string()))?;
    let data = vfs
        .read(&path)
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    Ok((path, data))
}

/// Loads an image with its full mip chain: the levels stored in a KTX file when they form a
/// complete chain down to 1x1, otherwise a CPU generated chain.
pub fn load_mip_chain(vfs: &Vfs, name: &str) -> Result<Vec<Rgba8Image>, ImageError> {
    if let Some(img) = builtin_image(name) {
        return Ok(img.mip_chain());
    }
    let (path, data) = read_image(vfs, name)?;
    if data.starts_with(&KTX_MAGIC) {
        let levels = decode_ktx_levels(&data)?;
        let complete = levels.len() > 1
            && levels.iter().enumerate().all(|(i, l)| {
                l.width == (levels[0].width >> i).max(1)
                    && l.height == (levels[0].height >> i).max(1)
            })
            && levels.last().is_some_and(|l| l.width == 1 && l.height == 1);
        if complete {
            return Ok(levels);
        }
        return Ok(levels[0].mip_chain());
    }
    Ok(Rgba8Image::decode_hint(&data, path.rsplit_once('.').map(|(_, e)| e))?.mip_chain())
}

const KTX_MAGIC: [u8; 12] = *b"\xABKTX 11\xBB\r\n\x1A\n";

/// Decodes every mip level stored in a KTX1 file (at least the first).
pub fn decode_ktx_levels(data: &[u8]) -> Result<Vec<Rgba8Image>, ImageError> {
    let err = |m: &str| ImageError::Decode(format!("ktx: {m}"));
    if data.len() < 64 || data[..12] != KTX_MAGIC {
        return Err(err("bad header"));
    }
    let word = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    if word(12) != 0x0403_0201 {
        return Err(err("big-endian files are not supported"));
    }
    let (gl_type, gl_format, internal) = (word(16), word(24), word(28));
    let (width, height) = (word(36) as usize, word(40) as usize);
    let (faces, levels, kv) = (word(52), word(56).max(1) as usize, word(60) as usize);
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(err("bad dimensions"));
    }
    if faces > 1 || word(44) > 1 || word(48) > 1 {
        return Err(err("cube, 3D and array textures are not supported"));
    }
    let mut offset = 64usize
        .checked_add(kv)
        .ok_or_else(|| err("bad key/value size"))?;
    let mut out = Vec::new();
    for level in 0..levels {
        let (w, h) = ((width >> level).max(1), (height >> level).max(1));
        if data.len() < offset + 4 {
            if level > 0 {
                break;
            }
            return Err(err("truncated"));
        }
        let size = word(offset) as usize;
        let Some(bytes) = data.get(offset + 4..offset + 4 + size) else {
            if level > 0 {
                break;
            }
            return Err(err("truncated image data"));
        };
        let decoded = if gl_type == 0 {
            decode_compressed(internal, bytes, w, h)
        } else {
            decode_uncompressed(gl_type, gl_format, bytes, w, h)
        };
        let pixels = match decoded {
            Ok(p) => p,
            Err(_) if level > 0 => break,
            Err(m) => return Err(err(&m)),
        };
        out.push(Rgba8Image {
            width: w as u32,
            height: h as u32,
            pixels,
        });
        offset += 4 + size.div_ceil(4) * 4;
    }
    Ok(out)
}

fn decode_compressed(internal: u32, blocks: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let mut out = vec![0u32; w * h];
    match internal {
        0x8D64 | 0x8D65 => texture2ddecoder::decode_etc1(blocks, w, h, &mut out),
        0x9274 | 0x9275 => texture2ddecoder::decode_etc2_rgb(blocks, w, h, &mut out),
        0x9278 | 0x9279 => texture2ddecoder::decode_etc2_rgba8(blocks, w, h, &mut out),
        0x83F0 | 0x83F1 => texture2ddecoder::decode_bc1(blocks, w, h, &mut out),
        0x83F2 => texture2ddecoder::decode_bc2(blocks, w, h, &mut out),
        0x83F3 => texture2ddecoder::decode_bc3(blocks, w, h, &mut out),
        other => {
            return Err(format!(
                "unsupported compressed internal format 0x{other:X}"
            ));
        }
    }
    .map_err(str::to_string)?;
    // The decoder packs pixels as 0xAARRGGBB.
    Ok(out
        .iter()
        .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8])
        .collect())
}

/// 8-bit-per-channel uncompressed data; rows are padded to 4 bytes (GL unpack alignment).
/// Luminance replicates into RGB and alpha-only images are white, as OpenGL samples them.
fn decode_uncompressed(
    gl_type: u32,
    gl_format: u32,
    bytes: &[u8],
    w: usize,
    h: usize,
) -> Result<Vec<u8>, String> {
    if gl_type != 0x1401 {
        return Err(format!("unsupported pixel type 0x{gl_type:X}"));
    }
    let bpp = match gl_format {
        0x1906 | 0x1909 | 0x8049 => 1, // ALPHA, LUMINANCE
        0x190A => 2,                   // LUMINANCE_ALPHA
        0x1907 | 0x80E0 => 3,          // RGB, BGR
        0x1908 | 0x80E1 => 4,          // RGBA, BGRA
        other => return Err(format!("unsupported pixel format 0x{other:X}")),
    };
    let padded = (w * bpp).div_ceil(4) * 4;
    // Some exporters write tightly packed rows even when the width is not a multiple of 4.
    let stride = if bytes.len() >= padded * (h - 1) + w * bpp {
        padded
    } else {
        w * bpp
    };
    if bytes.len() < stride * (h - 1) + w * bpp {
        return Err("truncated image data".into());
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for px in bytes[y * stride..y * stride + w * bpp].chunks_exact(bpp) {
            out.extend_from_slice(&match gl_format {
                0x1906 => [255, 255, 255, px[0]],
                0x1909 | 0x8049 => [px[0], px[0], px[0], 255],
                0x190A => [px[0], px[0], px[0], px[1]],
                0x1907 => [px[0], px[1], px[2], 255],
                0x80E0 => [px[2], px[1], px[0], 255],
                0x1908 => [px[0], px[1], px[2], px[3]],
                _ => [px[2], px[1], px[0], px[3]],
            });
        }
    }
    Ok(out)
}

/// Minimal TGA reader (types 2/3 raw, 10/11 RLE; 8, 24 and 32 bpp) that ignores the
/// descriptor's alpha-bit count, which some stock assets set on 24-bit data.
pub fn decode_tga(data: &[u8]) -> Result<Rgba8Image, ImageError> {
    let err = |m: &str| ImageError::Decode(format!("tga: {m}"));
    if data.len() < 18 {
        return Err(err("truncated header"));
    }
    let id_len = data[0] as usize;
    let (kind, bpp) = (data[2], data[16] as usize);
    let (w, h) = (
        u16::from_le_bytes([data[12], data[13]]) as usize,
        u16::from_le_bytes([data[14], data[15]]) as usize,
    );
    let rle = match kind {
        2 | 3 => false,
        10 | 11 => true,
        _ => return Err(err("unsupported image type")),
    };
    if w == 0 || h == 0 || !matches!(bpp, 8 | 24 | 32) || (kind & 3 == 3) != (bpp == 8) {
        return Err(err("unsupported dimensions or depth"));
    }
    let bytes = bpp / 8;
    let mut pos = 18 + id_len;
    let mut raw = Vec::with_capacity(w * h * bytes);
    if rle {
        while raw.len() < w * h * bytes {
            let head = *data.get(pos).ok_or_else(|| err("truncated data"))?;
            pos += 1;
            let count = (head & 0x7F) as usize + 1;
            if head & 0x80 != 0 {
                let px = data
                    .get(pos..pos + bytes)
                    .ok_or_else(|| err("truncated data"))?;
                pos += bytes;
                for _ in 0..count {
                    raw.extend_from_slice(px);
                }
            } else {
                let run = data
                    .get(pos..pos + count * bytes)
                    .ok_or_else(|| err("truncated data"))?;
                pos += count * bytes;
                raw.extend_from_slice(run);
            }
        }
        raw.truncate(w * h * bytes);
    } else {
        raw.extend_from_slice(
            data.get(pos..pos + w * h * bytes)
                .ok_or_else(|| err("truncated data"))?,
        );
    }
    let top_first = data[17] & 0x20 != 0;
    let mut pixels = vec![0u8; w * h * 4];
    for y in 0..h {
        let dst_y = if top_first { y } else { h - 1 - y };
        for x in 0..w {
            let s = &raw[(y * w + x) * bytes..][..bytes];
            let d = &mut pixels[(dst_y * w + x) * 4..][..4];
            d.copy_from_slice(&match bytes {
                1 => [s[0], s[0], s[0], 255],
                3 => [s[2], s[1], s[0], 255],
                _ => [s[2], s[1], s[0], s[3]],
            });
        }
    }
    Ok(Rgba8Image {
        width: w as u32,
        height: h as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chain_reaches_one_by_one() {
        let img = Rgba8Image::checker(8, 2);
        let chain = img.mip_chain();
        let sizes: Vec<_> = chain.iter().map(|m| m.width).collect();
        assert_eq!(sizes, [8, 4, 2, 1]);
        assert_eq!(chain[3].pixels.len(), 4);
    }

    #[test]
    fn non_square_mips_clamp_at_one() {
        let img = Rgba8Image {
            width: 4,
            height: 1,
            pixels: vec![255; 16],
        };
        let sizes: Vec<_> = img
            .mip_chain()
            .iter()
            .map(|m| (m.width, m.height))
            .collect();
        assert_eq!(sizes, [(4, 1), (2, 1), (1, 1)]);
    }

    #[test]
    fn decodes_png_roundtrip() {
        use image::ImageEncoder;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&[10, 20, 30, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let img = Rgba8Image::decode(&png).unwrap();
        assert_eq!(img.pixels, [10, 20, 30, 255]);
    }

    #[test]
    fn builtin_images_resolve_without_vfs() {
        let vfs = Vfs::new();
        assert_eq!(load_image(&vfs, "$whiteimage").unwrap().pixels, [255; 4]);
        assert_eq!(load_image(&vfs, "*black").unwrap().pixels, [0, 0, 0, 255]);
        assert!(load_mip_chain(&vfs, "*grey").is_ok());
        assert!(matches!(
            load_image(&vfs, "nope"),
            Err(ImageError::NotFound(_))
        ));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Rgba8Image::decode(b"nope").is_err());
    }

    fn ktx(gl_type: u32, format: u32, internal: u32, w: u32, h: u32, levels: &[&[u8]]) -> Vec<u8> {
        let mut v = KTX_MAGIC.to_vec();
        for x in [
            0x0403_0201u32,
            gl_type,
            1,
            format,
            internal,
            format,
            w,
            h,
            0,
            0,
            1,
            levels.len() as u32,
            0,
        ] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        for l in levels {
            v.extend_from_slice(&(l.len() as u32).to_le_bytes());
            v.extend_from_slice(l);
            v.resize(v.len().div_ceil(4) * 4, 0);
        }
        v
    }

    #[test]
    fn ktx_luminance_alpha_with_mips() {
        let data = ktx(
            0x1401,
            0x190A,
            0x8045,
            2,
            2,
            &[&[10, 20, 30, 40, 50, 60, 70, 80], &[9, 8]],
        );
        let lv = decode_ktx_levels(&data).unwrap();
        assert_eq!(lv.len(), 2);
        assert_eq!(&lv[0].pixels[..4], &[10, 10, 10, 20]);
        assert_eq!(&lv[1].pixels, &[9, 9, 9, 8]);
        assert_eq!(Rgba8Image::decode(&data).unwrap().width, 2);
    }

    #[test]
    fn ktx_alpha8_is_white_with_alpha_and_pads_rows() {
        // 3x2 ALPHA8: rows padded to 4 bytes.
        let data = ktx(0x1401, 0x1906, 0x803C, 3, 2, &[&[1, 2, 3, 0, 4, 5, 6, 0]]);
        let img = decode_ktx_levels(&data).unwrap().remove(0);
        assert_eq!(&img.pixels[..4], &[255, 255, 255, 1]);
        assert_eq!(&img.pixels[20..], &[255, 255, 255, 6]);
    }

    #[test]
    fn ktx_tolerates_tightly_packed_rows_and_truncated_chain() {
        // 2x2 level written without row padding, followed by a bogus level.
        let mut data = ktx(0x1401, 0x1906, 0x803C, 2, 2, &[&[1, 2, 3, 4]]);
        data[56..60].copy_from_slice(&3u32.to_le_bytes());
        data.extend_from_slice(&999u32.to_le_bytes());
        let lv = decode_ktx_levels(&data).unwrap();
        assert_eq!(lv.len(), 1);
        assert_eq!(lv[0].pixels[15], 4);
    }

    #[test]
    fn ktx_is_sniffed_regardless_of_name() {
        let data = ktx(0x1401, 0x1908, 0x8058, 1, 1, &[&[1, 2, 3, 4]]);
        assert_eq!(Rgba8Image::decode(&data).unwrap().pixels, [1, 2, 3, 4]);
    }

    #[test]
    fn tga_with_alpha_descriptor_on_24bpp() {
        let mut t = vec![0u8; 18];
        t[2] = 2;
        t[12] = 1;
        t[14] = 1;
        t[16] = 24;
        t[17] = 0x08;
        t.extend_from_slice(&[3, 2, 1]); // BGR
        assert_eq!(decode_tga(&t).unwrap().pixels, [1, 2, 3, 255]);
    }

    #[test]
    fn tga_rle_32bpp() {
        let mut t = vec![0u8; 18];
        t[2] = 10;
        t[12] = 2;
        t[14] = 1;
        t[16] = 32;
        t[17] = 0x28; // top-left origin
        t.extend_from_slice(&[0x81, 3, 2, 1, 9]);
        let img = decode_tga(&t).unwrap();
        assert_eq!(img.pixels, [1, 2, 3, 9, 1, 2, 3, 9]);
    }
}
