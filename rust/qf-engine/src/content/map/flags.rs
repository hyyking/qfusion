//! Canonical contents and surface flags of the map IR.
//!
//! The numeric values equal the Q3/FBSP lump values, so the FBSP converter
//! copies them unchanged and collision stays bit-identical.

pub const CONTENTS_SOLID: u32 = 1;
pub const CONTENTS_LAVA: u32 = 8;
pub const CONTENTS_SLIME: u32 = 16;
pub const CONTENTS_WATER: u32 = 32;
pub const CONTENTS_PLAYERCLIP: u32 = 0x10000;
pub const CONTENTS_BODY: u32 = 0x200_0000;

pub const MASK_PLAYERSOLID: u32 = CONTENTS_SOLID | CONTENTS_PLAYERCLIP | CONTENTS_BODY;
pub const MASK_DEADSOLID: u32 = CONTENTS_SOLID | CONTENTS_PLAYERCLIP;
pub const MASK_WATER: u32 = CONTENTS_WATER | CONTENTS_LAVA | CONTENTS_SLIME;

pub const SURF_NODAMAGE: u32 = 0x1;
pub const SURF_SLICK: u32 = 0x2;
pub const SURF_SKY: u32 = 0x4;
pub const SURF_LADDER: u32 = 0x8;
pub const SURF_NODRAW: u32 = 0x80;
pub const SURF_SKIP: u32 = 0x200;
pub const SURF_NONSOLID: u32 = 0x4000;
pub const SURF_NOWALLJUMP: u32 = 0x8_0000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_match_fbsp() {
        assert_eq!(
            [
                CONTENTS_SOLID,
                CONTENTS_LAVA,
                CONTENTS_SLIME,
                CONTENTS_WATER,
                CONTENTS_PLAYERCLIP,
                CONTENTS_BODY
            ],
            [1, 8, 16, 32, 0x10000, 0x200_0000]
        );
        assert_eq!(
            [
                SURF_NODAMAGE,
                SURF_SLICK,
                SURF_SKY,
                SURF_LADDER,
                SURF_NODRAW,
                SURF_SKIP,
                SURF_NONSOLID,
                SURF_NOWALLJUMP
            ],
            [0x1, 0x2, 0x4, 0x8, 0x80, 0x200, 0x4000, 0x8_0000]
        );
        assert_eq!(MASK_PLAYERSOLID, 0x201_0001);
        assert_eq!(MASK_DEADSOLID, 0x1_0001);
        assert_eq!(MASK_WATER, 56);
    }
}
