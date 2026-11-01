use crate::content::bsp::Bsp;

use super::pmove::{CONTENTS_PLAYERCLIP, CONTENTS_SOLID, MASK_WATER, PmTrace, PmoveWorld, Vec3};

/// Movement collision against world model 0 of a parsed BSP.
///
/// Only brush collision is available: patch faces and inline submodels are
/// not collidable yet, and every hit reports entity 0 (the world).
pub struct BspWorld<'a> {
    bsp: &'a Bsp,
}

impl<'a> BspWorld<'a> {
    pub fn new(bsp: &'a Bsp) -> Self {
        Self { bsp }
    }
}

impl PmoveWorld for BspWorld<'_> {
    fn trace(&self, start: Vec3, mins: Vec3, maxs: Vec3, end: Vec3, contents_mask: u32) -> PmTrace {
        let Ok(trace) = self.bsp.trace(start, end, mins, maxs, contents_mask) else {
            return PmTrace::miss(end);
        };
        let hit = trace.fraction < 1.0 || trace.start_solid;
        PmTrace {
            fraction: trace.fraction,
            end_position: trace.end_position,
            plane_normal: trace.plane.map_or([0.0; 3], |plane| plane.normal),
            surface_flags: trace.surface_flags,
            contents: trace.contents,
            entity: if hit { 0 } else { -1 },
            start_solid: trace.start_solid,
            all_solid: trace.all_solid,
        }
    }

    fn point_contents(&self, point: Vec3) -> u32 {
        let inside = |mask: u32| {
            self.bsp
                .trace(point, point, [0.0; 3], [0.0; 3], mask)
                .map_or(
                    0,
                    |trace| if trace.start_solid { trace.contents } else { 0 },
                )
        };
        inside(MASK_WATER) | inside(CONTENTS_SOLID | CONTENTS_PLAYERCLIP)
    }
}
