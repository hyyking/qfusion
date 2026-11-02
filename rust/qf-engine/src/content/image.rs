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
        if ext.is_some_and(|e| e.eq_ignore_ascii_case("ktx")) {
            return Self::decode_ktx(data);
        }
        let format = ext.and_then(image::ImageFormat::from_extension);
        let img = match format {
            Some(f) => image::load_from_memory_with_format(data, f),
            None => image::load_from_memory(data),
        }
        .map_err(|e| ImageError::Decode(e.to_string()))?;
        let rgba = img.to_rgba8();
        Ok(Self {
            width: rgba.width(),
            height: rgba.height(),
            pixels: rgba.into_raw(),
        })
    }

    /// KTX1 container with a block-compressed first mip (legacy ships ETC1 `.ktx` textures
    /// for some assets); decoded with `texture2ddecoder`.
    fn decode_ktx(data: &[u8]) -> Result<Self, ImageError> {
        const MAGIC: [u8; 12] = *b"\xABKTX 11\xBB\r\n\x1A\n";
        let err = |m: &str| ImageError::Decode(format!("ktx: {m}"));
        if data.len() < 68 || data[..12] != MAGIC {
            return Err(err("bad header"));
        }
        let word = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        if word(12) != 0x0403_0201 {
            return Err(err("big-endian files are not supported"));
        }
        let (internal, width, height, kv) = (
            word(28),
            word(36) as usize,
            word(40) as usize,
            word(60) as usize,
        );
        let start = 64 + kv;
        if width == 0 || height == 0 || data.len() < start + 4 {
            return Err(err("truncated"));
        }
        let size = word(start) as usize;
        let blocks = data
            .get(start + 4..start + 4 + size)
            .ok_or_else(|| err("truncated image data"))?;
        let mut out = vec![0u32; width * height];
        let result = match internal {
            0x8D64 | 0x8D65 => texture2ddecoder::decode_etc1(blocks, width, height, &mut out),
            0x9274 | 0x9275 => texture2ddecoder::decode_etc2_rgb(blocks, width, height, &mut out),
            0x9278 | 0x9279 => texture2ddecoder::decode_etc2_rgba8(blocks, width, height, &mut out),
            0x83F0 | 0x83F1 => texture2ddecoder::decode_bc1(blocks, width, height, &mut out),
            0x83F2 => texture2ddecoder::decode_bc2(blocks, width, height, &mut out),
            0x83F3 => texture2ddecoder::decode_bc3(blocks, width, height, &mut out),
            other => return Err(err(&format!("unsupported internal format 0x{other:X}"))),
        };
        result.map_err(err)?;
        // The decoder packs pixels as 0xAARRGGBB.
        let pixels = out
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8])
            .collect();
        Ok(Self {
            width: width as u32,
            height: height as u32,
            pixels,
        })
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

pub fn load_image(vfs: &Vfs, name: &str) -> Result<Rgba8Image, ImageError> {
    let path = find_image(vfs, name).ok_or_else(|| ImageError::NotFound(name.to_string()))?;
    let data = vfs
        .read(&path)
        .map_err(|e| ImageError::Decode(e.to_string()))?;
    Rgba8Image::decode_hint(&data, path.rsplit_once('.').map(|(_, e)| e))
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
    fn rejects_garbage() {
        assert!(Rgba8Image::decode(b"nope").is_err());
    }
}
