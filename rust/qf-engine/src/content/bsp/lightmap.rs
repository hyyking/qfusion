//! BSP lightmap lump decoding and atlas packing.
//!
//! Layout follows `Mod_LoadLighting`/`R_BuildLightmaps` in the legacy renderer: the lump is a
//! sequence of `LIGHTMAP_BYTES`(3)-per-texel RGB blocks (128x128 for IBSP, 512x512 for FBSP).
//! When a map carries deluxemaps, blocks alternate lightmap/deluxemap and only the even ones
//! are colour data. Light styles are ignored (style 0 only).

/// Largest atlas edge we pack to; matches the default wgpu texture limit.
pub const MAX_ATLAS_SIZE: usize = 8192;

#[derive(Debug, Clone, Default)]
pub struct LightmapAtlas {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major; empty when the map has no usable lightmaps.
    pub rgba: Vec<u8>,
    pub deluxe: bool,
    /// Per lump block: `[scale_x, scale_y, offset_x, offset_y]` mapping block-local
    /// coordinates into atlas coordinates, or `None` for unusable blocks.
    pub rects: Vec<Option<[f32; 4]>>,
    pub dropped_blocks: usize,
}

impl LightmapAtlas {
    pub fn is_empty(&self) -> bool {
        self.rgba.is_empty()
    }

    /// Builds the atlas from the lighting lump. `face_blocks` are the lightmap block numbers
    /// referenced by faces (negative = none), used to detect deluxemaps.
    pub fn build(data: &[u8], block: usize, face_blocks: impl Iterator<Item = i32>) -> Self {
        let block_bytes = block * block * 3;
        if data.is_empty() || !data.len().is_multiple_of(block_bytes) {
            return Self::default();
        }
        let blocks = data.len() / block_bytes;
        let deluxe = blocks >= 2
            && blocks.is_multiple_of(2)
            && face_blocks.into_iter().all(|b| b <= 0 || b % 2 == 0)
            && (blocks != 2 || data[block_bytes..].iter().any(|&b| b != 0));
        let step = if deluxe { 2 } else { 1 };
        let wanted = blocks.div_ceil(step);

        let max_cols = MAX_ATLAS_SIZE / block;
        let mut cols = ((wanted as f64).sqrt().ceil() as usize).clamp(1, max_cols);
        let mut rows = wanted.div_ceil(cols);
        let capacity = if rows * block > MAX_ATLAS_SIZE {
            rows = MAX_ATLAS_SIZE / block;
            cols = max_cols;
            rows * cols
        } else {
            wanted
        };
        let (width, height) = (cols * block, rows * block);
        let mut rgba = vec![255u8; width * height * 4];
        let mut rects = vec![None; blocks];
        for slot in 0..capacity.min(wanted) {
            let b = slot * step;
            let (cx, cy) = (slot % cols, slot / cols);
            let src = &data[b * block_bytes..(b + 1) * block_bytes];
            for y in 0..block {
                let dst = ((cy * block + y) * width + cx * block) * 4;
                for x in 0..block {
                    let s = (y * block + x) * 3;
                    rgba[dst + x * 4..dst + x * 4 + 3].copy_from_slice(&src[s..s + 3]);
                }
            }
            rects[b] = Some([
                1.0 / cols as f32,
                1.0 / rows as f32,
                cx as f32 / cols as f32,
                cy as f32 / rows as f32,
            ]);
        }
        Self {
            width: width as u32,
            height: height as u32,
            rgba,
            deluxe,
            rects,
            dropped_blocks: wanted.saturating_sub(capacity),
        }
    }

    /// Maps block-local lightmap coordinates into the atlas; `[-1, -1]` marks "no lightmap".
    pub fn map_uv(&self, block: i32, uv: [f32; 2]) -> [f32; 2] {
        let rect = usize::try_from(block)
            .ok()
            .and_then(|b| self.rects.get(b))
            .and_then(|r| r.as_ref());
        match rect {
            Some(&[sx, sy, ox, oy]) => [uv[0] * sx + ox, uv[1] * sy + oy],
            None => [-1.0, -1.0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lump(blocks: usize, block: usize, fill: impl Fn(usize) -> u8) -> Vec<u8> {
        (0..blocks)
            .flat_map(|b| std::iter::repeat_n(fill(b), block * block * 3))
            .collect()
    }

    #[test]
    fn packs_blocks_into_grid_and_maps_uvs() {
        let data = lump(3, 4, |b| (b as u8 + 1) * 10);
        let atlas = LightmapAtlas::build(&data, 4, [0, 1, 2].into_iter());
        assert!(!atlas.deluxe);
        assert_eq!((atlas.width, atlas.height), (8, 8));
        // Block 1 sits in the second column of the first row.
        assert_eq!(&atlas.rgba[4 * 4..4 * 4 + 4], &[20, 20, 20, 255]);
        assert_eq!(atlas.map_uv(1, [1.0, 1.0]), [1.0, 0.5]);
        assert_eq!(atlas.map_uv(2, [0.0, 0.0]), [0.0, 0.5]);
        assert_eq!(atlas.map_uv(-1, [0.5, 0.5]), [-1.0, -1.0]);
        assert_eq!(atlas.map_uv(9, [0.5, 0.5]), [-1.0, -1.0]);
    }

    #[test]
    fn deluxemap_blocks_are_skipped() {
        let data = lump(4, 2, |b| b as u8 + 1);
        let atlas = LightmapAtlas::build(&data, 2, [0, 2].into_iter());
        assert!(atlas.deluxe);
        assert_eq!((atlas.width, atlas.height), (4, 2));
        assert!(atlas.rects[1].is_none() && atlas.rects[3].is_none());
        assert_eq!(atlas.rgba[2 * 4], 3, "block 2 is the second colour block");
    }

    #[test]
    fn empty_or_misaligned_lump_yields_empty_atlas() {
        assert!(LightmapAtlas::build(&[], 4, std::iter::empty()).is_empty());
        assert!(LightmapAtlas::build(&[1, 2, 3], 4, std::iter::empty()).is_empty());
    }
}
