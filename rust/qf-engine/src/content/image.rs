//! GPU-independent image decoding (tga/jpg/png) and mip chain generation.

use std::fmt;

use super::vfs::Vfs;

/// Extensions tried, in legacy order, when a texture name has none or a missing one.
const EXTENSIONS: [&str; 3] = ["tga", "jpg", "png"];

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
        let img = image::load_from_memory(data).map_err(|e| ImageError::Decode(e.to_string()))?;
        let rgba = img.to_rgba8();
        Ok(Self {
            width: rgba.width(),
            height: rgba.height(),
            pixels: rgba.into_raw(),
        })
    }

    /// Solid-colour checkerboard used where an asset is missing.
    pub fn checker(size: u32, cell: u32) -> Self {
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let on = ((x / cell) + (y / cell)) % 2 == 0;
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
                let ys = [(y * 2).min(self.height - 1), (y * 2 + 1).min(self.height - 1)];
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
    Rgba8Image::decode(&data)
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
        let sizes: Vec<_> = img.mip_chain().iter().map(|m| (m.width, m.height)).collect();
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
