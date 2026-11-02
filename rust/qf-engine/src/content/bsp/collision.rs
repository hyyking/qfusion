use super::patch::PatchCollide;
use super::{Brush, Bsp, Plane};

const DIST_EPSILON: f32 = 1.0 / 32.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub fraction: f32,
    pub end_position: [f32; 3],
    pub plane: Option<TracePlane>,
    pub contents: u32,
    pub surface_flags: u32,
    pub shader: Option<usize>,
    pub start_solid: bool,
    pub all_solid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TracePlane {
    pub normal: [f32; 3],
    pub dist: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    NonFiniteInput,
    InvertedBounds,
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TraceError {}

/// Placement of an inline model: translation plus an optional rotation.
///
/// `axis` holds the rows of the world-to-model rotation (`AnglesToAxis`:
/// forward, left, up). `None` means unrotated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPose {
    pub origin: [f32; 3],
    pub axis: Option<[[f32; 3]; 3]>,
}

impl ModelPose {
    pub fn translated(origin: [f32; 3]) -> Self {
        Self { origin, axis: None }
    }

    fn world_to_local(&self, point: [f32; 3]) -> [f32; 3] {
        let shifted = std::array::from_fn(|axis| point[axis] - self.origin[axis]);
        match self.axis {
            Some(rows) => std::array::from_fn(|row| dot(rows[row], shifted)),
            None => shifted,
        }
    }
}

#[derive(Clone, Copy)]
struct Segment {
    node: i32,
    start_fraction: f32,
    end_fraction: f32,
    start: [f32; 3],
    end: [f32; 3],
}

impl Bsp {
    /// Sweeps a point or axis-aligned box through world model 0's BSP brushes.
    ///
    /// `mins` and `maxs` are offsets from the trace origin. Solid patch faces are
    /// clipped via their collision facets; inline submodels are not included.
    pub fn trace(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        contents_mask: u32,
    ) -> Result<Trace, TraceError> {
        if start
            .iter()
            .chain(end.iter())
            .chain(mins.iter())
            .chain(maxs.iter())
            .any(|x| !x.is_finite())
        {
            return Err(TraceError::NonFiniteInput);
        }
        if (0..3).any(|axis| mins[axis] > maxs[axis]) {
            return Err(TraceError::InvertedBounds);
        }

        let mut result = Trace {
            fraction: 1.0,
            end_position: end,
            plane: None,
            contents: 0,
            surface_flags: 0,
            shader: None,
            start_solid: false,
            all_solid: false,
        };
        if self.nodes.is_empty() || contents_mask == 0 {
            return Ok(result);
        }

        if start == end {
            return Ok(self.position_test(start, mins, maxs, contents_mask, result));
        }

        let extents = std::array::from_fn(|axis| (-mins[axis]).max(maxs[axis]));
        let mut seen_brushes = vec![false; self.brushes.len()];
        let mut seen_patches = vec![false; self.patches.len()];
        let mut stack = vec![Segment {
            node: 0,
            start_fraction: 0.0,
            end_fraction: 1.0,
            start,
            end,
        }];

        while let Some(segment) = stack.pop() {
            if result.fraction <= segment.start_fraction {
                continue;
            }
            if segment.node < 0 {
                let leaf_index = (-i64::from(segment.node) - 1) as usize;
                let leaf = &self.leafs[leaf_index];
                for &brush_index in &self.leaf_brushes
                    [leaf.first_leaf_brush..leaf.first_leaf_brush + leaf.num_leaf_brushes]
                {
                    if seen_brushes[brush_index] {
                        continue;
                    }
                    seen_brushes[brush_index] = true;
                    let brush = &self.brushes[brush_index];
                    if brush.contents as u32 & contents_mask != 0 {
                        self.clip_brush(brush, start, end, mins, maxs, &mut result);
                        if result.all_solid {
                            break;
                        }
                    }
                }
                if result.all_solid {
                    continue;
                }
                for &face_index in &self.leaf_faces
                    [leaf.first_leaf_face..leaf.first_leaf_face + leaf.num_leaf_faces]
                {
                    let Some(patch_index) = self.face_patch[face_index] else {
                        continue;
                    };
                    if seen_patches[patch_index] {
                        continue;
                    }
                    seen_patches[patch_index] = true;
                    let patch = &self.patches[patch_index];
                    if patch.contents as u32 & contents_mask != 0 {
                        self.clip_patch(patch, (start, end, mins, maxs), &mut result);
                        if result.all_solid {
                            break;
                        }
                    }
                }
                continue;
            }

            let node = &self.nodes[segment.node as usize];
            let plane = &self.planes[node.plane];
            let t1 = dot(plane.normal, segment.start) - plane.dist;
            let t2 = dot(plane.normal, segment.end) - plane.dist;
            let offset = dot_abs(plane.normal, extents);

            if t1 >= offset && t2 >= offset {
                stack.push(Segment {
                    node: node.children[0],
                    ..segment
                });
                continue;
            }
            if t1 < -offset && t2 < -offset {
                stack.push(Segment {
                    node: node.children[1],
                    ..segment
                });
                continue;
            }

            let (side, near_fraction, far_fraction) = if t1 < t2 {
                let inverse = 1.0 / (t1 - t2);
                (
                    1,
                    (t1 - offset + DIST_EPSILON) * inverse,
                    (t1 + offset + DIST_EPSILON) * inverse,
                )
            } else if t1 > t2 {
                let inverse = 1.0 / (t1 - t2);
                (
                    0,
                    (t1 + offset + DIST_EPSILON) * inverse,
                    (t1 - offset - DIST_EPSILON) * inverse,
                )
            } else {
                (0, 1.0, 0.0)
            };
            let near_fraction = near_fraction.clamp(0.0, 1.0);
            let far_fraction = far_fraction.clamp(0.0, 1.0);
            let near_mid = lerp(segment.start, segment.end, near_fraction);
            let far_mid = lerp(segment.start, segment.end, far_fraction);
            let mid_fraction = segment.start_fraction
                + (segment.end_fraction - segment.start_fraction) * near_fraction;
            let far_mid_fraction = segment.start_fraction
                + (segment.end_fraction - segment.start_fraction) * far_fraction;

            stack.push(Segment {
                node: node.children[side ^ 1],
                start_fraction: far_mid_fraction,
                end_fraction: segment.end_fraction,
                start: far_mid,
                end: segment.end,
            });
            stack.push(Segment {
                node: node.children[side],
                start_fraction: segment.start_fraction,
                end_fraction: mid_fraction,
                start: segment.start,
                end: near_mid,
            });
        }

        if result.fraction < 1.0 {
            result.end_position = lerp(start, end, result.fraction);
        }
        Ok(result)
    }

    /// The legacy `start == end` special case: no sweep, just whether the box overlaps solid
    /// geometry. Every leaf within one unit of the box is tested and touching faces count as
    /// inside, so a box flush against a wall is start-solid (a swept trace would skip the leaf).
    fn position_test(
        &self,
        origin: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        contents_mask: u32,
        mut result: Trace,
    ) -> Trace {
        let box_mins: [f32; 3] = std::array::from_fn(|axis| origin[axis] + mins[axis] - 1.0);
        let box_maxs: [f32; 3] = std::array::from_fn(|axis| origin[axis] + maxs[axis] + 1.0);
        let center: [f32; 3] = std::array::from_fn(|axis| (box_mins[axis] + box_maxs[axis]) * 0.5);
        let half: [f32; 3] = std::array::from_fn(|axis| (box_maxs[axis] - box_mins[axis]) * 0.5);

        let mut stack = vec![0i32];
        let mut leaves = Vec::new();
        while let Some(node) = stack.pop() {
            if node < 0 {
                leaves.push((-i64::from(node) - 1) as usize);
                continue;
            }
            let node = &self.nodes[node as usize];
            let plane = &self.planes[node.plane];
            let distance = dot(plane.normal, center) - plane.dist;
            let offset = dot_abs(plane.normal, half);
            if distance >= offset {
                stack.push(node.children[0]);
            } else if distance < -offset {
                stack.push(node.children[1]);
            } else {
                stack.push(node.children[1]);
                stack.push(node.children[0]);
            }
        }

        let mut seen_brushes = vec![false; self.brushes.len()];
        let mut seen_patches = vec![false; self.patches.len()];
        let solid = |planes: &mut dyn Iterator<Item = &Plane>| {
            for plane in planes {
                let support = support_point(origin, mins, maxs, plane.normal);
                if dot(plane.normal, support) > plane.dist {
                    return false;
                }
            }
            true
        };
        for leaf_index in leaves {
            let leaf = &self.leafs[leaf_index];
            for &brush_index in &self.leaf_brushes
                [leaf.first_leaf_brush..leaf.first_leaf_brush + leaf.num_leaf_brushes]
            {
                if std::mem::replace(&mut seen_brushes[brush_index], true) {
                    continue;
                }
                let brush = &self.brushes[brush_index];
                if brush.contents as u32 & contents_mask == 0 || brush.num_sides == 0 {
                    continue;
                }
                let sides = &self.brush_sides[brush.first_side..brush.first_side + brush.num_sides];
                if solid(&mut sides.iter().map(|side| &self.planes[side.plane])) {
                    return Self::inside(result, origin, brush.contents as u32);
                }
            }
            for &face_index in
                &self.leaf_faces[leaf.first_leaf_face..leaf.first_leaf_face + leaf.num_leaf_faces]
            {
                let Some(patch_index) = self.face_patch[face_index] else {
                    continue;
                };
                if std::mem::replace(&mut seen_patches[patch_index], true) {
                    continue;
                }
                let patch = &self.patches[patch_index];
                if patch.contents as u32 & contents_mask == 0 {
                    continue;
                }
                for facet in &patch.facets {
                    if !facet.planes.is_empty() && solid(&mut facet.planes.iter()) {
                        return Self::inside(result, origin, patch.contents as u32);
                    }
                }
            }
        }
        result.end_position = origin;
        result
    }

    fn inside(mut trace: Trace, origin: [f32; 3], contents: u32) -> Trace {
        trace.start_solid = true;
        trace.all_solid = true;
        trace.fraction = 0.0;
        trace.contents = contents;
        trace.end_position = origin;
        trace
    }

    /// Tests whether an absolute-coordinate box touches an inline brush model.
    ///
    /// Mirrors the legacy `GClip_EntityContact` for brush models: a zero-length
    /// box trace against the model translated by `origin` (brushes and patch
    /// faces), where "start solid" means overlap (touching faces count).
    /// Model 0 is the world and is rejected.
    pub fn box_touches_model(
        &self,
        model: usize,
        origin: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        contents_mask: u32,
    ) -> Result<bool, TraceError> {
        if origin
            .iter()
            .chain(mins.iter())
            .chain(maxs.iter())
            .any(|x| !x.is_finite())
        {
            return Err(TraceError::NonFiniteInput);
        }
        if (0..3).any(|axis| mins[axis] > maxs[axis]) {
            return Err(TraceError::InvertedBounds);
        }
        let trace = self.trace_model(
            model,
            &ModelPose::translated(origin),
            ([0.0; 3], [0.0; 3], mins, maxs),
            contents_mask,
        );
        Ok(trace.is_ok_and(|t| t.start_solid))
    }

    /// Sweeps a box through an inline brush model placed at `pose`
    /// (`CM_TransformedBoxTrace`).
    ///
    /// The end points are moved into the model's frame, so the box itself is
    /// never rotated; the hit plane normal is rotated back to world space.
    /// Brushes and solid patch faces of the model are both clipped. Model 0
    /// and unknown models never hit.
    pub fn trace_model(
        &self,
        model: usize,
        pose: &ModelPose,
        sweep: ([f32; 3], [f32; 3], [f32; 3], [f32; 3]),
        contents_mask: u32,
    ) -> Result<Trace, TraceError> {
        let (start, end, mins, maxs) = sweep;
        if start
            .iter()
            .chain(end.iter())
            .chain(mins.iter())
            .chain(maxs.iter())
            .chain(pose.origin.iter())
            .any(|x| !x.is_finite())
        {
            return Err(TraceError::NonFiniteInput);
        }
        if (0..3).any(|axis| mins[axis] > maxs[axis]) {
            return Err(TraceError::InvertedBounds);
        }
        let mut result = Trace {
            fraction: 1.0,
            end_position: end,
            plane: None,
            contents: 0,
            surface_flags: 0,
            shader: None,
            start_solid: false,
            all_solid: false,
        };
        if model == 0 || contents_mask == 0 {
            return Ok(result);
        }
        let Some(model) = self.models.get(model) else {
            return Ok(result);
        };

        let start_local = pose.world_to_local(start);
        let end_local = pose.world_to_local(end);
        let local_sweep = (start_local, end_local, mins, maxs);
        self.clip_model(model, local_sweep, contents_mask, &mut result);

        if result.fraction < 1.0 {
            if let (Some(plane), Some(axis)) = (result.plane.as_mut(), pose.axis) {
                plane.normal = std::array::from_fn(|column| {
                    axis[0][column] * plane.normal[0]
                        + axis[1][column] * plane.normal[1]
                        + axis[2][column] * plane.normal[2]
                });
            }
            result.end_position = lerp(start, end, result.fraction);
        } else {
            result.end_position = end;
        }
        Ok(result)
    }

    fn clip_model(
        &self,
        model: &super::Model,
        sweep: ([f32; 3], [f32; 3], [f32; 3], [f32; 3]),
        contents_mask: u32,
        result: &mut Trace,
    ) {
        let (start, end, mins, maxs) = sweep;
        let (first, count) = (model.first_brush as usize, model.num_brushes as usize);
        if let Some(brushes) = self.brushes.get(first..first + count) {
            for brush in brushes {
                if brush.contents as u32 & contents_mask != 0 {
                    self.clip_brush(brush, start, end, mins, maxs, result);
                    if result.all_solid {
                        return;
                    }
                }
            }
        }
        let (first, count) = (model.first_face as usize, model.num_faces as usize);
        let Some(faces) = self.face_patch.get(first..first + count) else {
            return;
        };
        for patch_index in faces.iter().flatten() {
            let patch = &self.patches[*patch_index];
            if patch.contents as u32 & contents_mask != 0 {
                self.clip_patch(patch, sweep, result);
                if result.all_solid {
                    return;
                }
            }
        }
    }

    fn clip_brush(
        &self,
        brush: &Brush,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        trace: &mut Trace,
    ) {
        let sides = &self.brush_sides[brush.first_side..brush.first_side + brush.num_sides];
        self.clip_planes(
            sides
                .iter()
                .map(|side| (&self.planes[side.plane], side.shader)),
            brush.contents,
            (start, end, mins, maxs),
            trace,
        );
    }

    fn clip_patch(
        &self,
        patch: &PatchCollide,
        sweep: ([f32; 3], [f32; 3], [f32; 3], [f32; 3]),
        trace: &mut Trace,
    ) {
        let (start, end, mins, maxs) = sweep;
        if !sweep_overlaps(sweep, patch.mins, patch.maxs) {
            return;
        }
        for facet in &patch.facets {
            if !sweep_overlaps(sweep, facet.mins, facet.maxs) {
                continue;
            }
            self.clip_planes(
                facet.planes.iter().map(|p| (p, patch.shader)),
                patch.contents,
                (start, end, mins, maxs),
                trace,
            );
            if trace.all_solid {
                return;
            }
        }
    }

    /// Clips the swept box against a convex set of planes (brush or patch facet).
    fn clip_planes<'a>(
        &self,
        sides: impl Iterator<Item = (&'a Plane, usize)>,
        contents: i32,
        sweep: ([f32; 3], [f32; 3], [f32; 3], [f32; 3]),
        trace: &mut Trace,
    ) {
        let (start, end, mins, maxs) = sweep;
        let mut enter_fraction = -1.0f32;
        let mut leave_fraction = 1.0f32;
        let mut lead_plane: Option<&Plane> = None;
        let mut lead_shader = None;
        let mut get_out = false;
        let mut start_out = false;
        let mut any_side = false;

        for (plane, shader) in sides {
            any_side = true;
            let start_support = support_point(start, mins, maxs, plane.normal);
            let end_support = support_point(end, mins, maxs, plane.normal);
            let d1 = dot(plane.normal, start_support) - plane.dist;
            let d2 = dot(plane.normal, end_support) - plane.dist;

            if d2 > 0.0 {
                get_out = true;
            }
            if d1 > 0.0 {
                start_out = true;
            }
            if d1 > 0.0 && d2 >= d1 {
                return;
            }
            if d1 <= 0.0 && d2 <= 0.0 {
                continue;
            }
            let difference = d1 - d2;
            if difference > 0.0 {
                let fraction = (d1 - DIST_EPSILON) * (1.0 / difference);
                if fraction > enter_fraction {
                    enter_fraction = fraction;
                    lead_plane = Some(plane);
                    lead_shader = Some(shader);
                }
            } else if difference < 0.0 {
                let fraction = (d1 + DIST_EPSILON) * (1.0 / difference);
                if fraction < leave_fraction {
                    leave_fraction = fraction;
                }
            }
        }

        if !any_side {
            return;
        }
        if !start_out {
            trace.start_solid = true;
            trace.contents = contents as u32;
            if !get_out {
                trace.all_solid = true;
                trace.fraction = 0.0;
            }
            return;
        }
        if enter_fraction - (1.0 / 1024.0) <= leave_fraction
            && enter_fraction > -1.0
            && enter_fraction < trace.fraction
        {
            let shader = lead_shader.expect("enter plane always has a leading brush side");
            let shader_ref = &self.shaders[shader];
            trace.fraction = enter_fraction.max(0.0);
            trace.plane = lead_plane.map(|p| TracePlane {
                normal: p.normal,
                dist: p.dist,
            });
            trace.surface_flags = shader_ref.surface_flags as u32;
            trace.contents = contents as u32;
            trace.shader = Some(shader);
        }
    }
}

/// Conservative test of the swept box (start..end grown by mins/maxs) against bounds.
fn sweep_overlaps(
    (start, end, mins, maxs): ([f32; 3], [f32; 3], [f32; 3], [f32; 3]),
    bounds_mins: [f32; 3],
    bounds_maxs: [f32; 3],
) -> bool {
    (0..3).all(|axis| {
        start[axis].min(end[axis]) + mins[axis] <= bounds_maxs[axis]
            && start[axis].max(end[axis]) + maxs[axis] >= bounds_mins[axis]
    })
}

fn support_point(origin: [f32; 3], mins: [f32; 3], maxs: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|axis| {
        origin[axis]
            + if normal[axis] < 0.0 {
                maxs[axis]
            } else {
                mins[axis]
            }
    })
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn dot_abs(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0].abs() * b[0] + a[1].abs() * b[1] + a[2].abs() * b[2]
}

fn lerp(a: [f32; 3], b: [f32; 3], fraction: f32) -> [f32; 3] {
    std::array::from_fn(|axis| a[axis] + fraction * (b[axis] - a[axis]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_all_leaf_brushes(
        bsp: &Bsp,
        start: [f32; 3],
        end: [f32; 3],
        mins: [f32; 3],
        maxs: [f32; 3],
        contents_mask: u32,
    ) -> Trace {
        let mut trace = Trace {
            fraction: 1.0,
            end_position: end,
            plane: None,
            contents: 0,
            surface_flags: 0,
            shader: None,
            start_solid: false,
            all_solid: false,
        };
        let mut seen = vec![false; bsp.brushes.len()];
        for leaf in &bsp.leafs {
            for &brush_index in &bsp.leaf_brushes
                [leaf.first_leaf_brush..leaf.first_leaf_brush + leaf.num_leaf_brushes]
            {
                if seen[brush_index] {
                    continue;
                }
                seen[brush_index] = true;
                let brush = &bsp.brushes[brush_index];
                if brush.contents as u32 & contents_mask != 0 {
                    bsp.clip_brush(brush, start, end, mins, maxs, &mut trace);
                }
            }
        }
        if trace.fraction < 1.0 {
            trace.end_position = lerp(start, end, trace.fraction);
        }
        trace
    }

    #[test]
    fn zero_length_trace_flush_against_wall_is_start_solid() {
        let basewsw = std::env::var("WARSOW_BASEWSW")
            .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
        let pk3 = std::path::Path::new(&basewsw).join("map_wbomb1.pk3");
        if !pk3.exists() {
            eprintln!("SKIPPED: {} not found", pk3.display());
            return;
        }
        let data = super::super::read_pk3_entry(&pk3, "maps/wbomb1.bsp").unwrap();
        let bsp = Bsp::parse(&data).unwrap();
        // Recorded wbomb1 player box touching the x = -1200 wall: legacy position test is solid.
        let at = [-1200.0, 245.77448, 252.97185];
        let flush = bsp
            .trace(at, at, [-16.0, -16.0, -24.0], [16.0, 16.0, 40.0], 1)
            .unwrap();
        assert!(flush.start_solid && flush.all_solid && flush.fraction == 0.0);
        let clear = [-1199.875, 245.77448, 252.97185];
        let free = bsp
            .trace(clear, clear, [-16.0, -16.0, -24.0], [16.0, 16.0, 40.0], 1)
            .unwrap();
        assert!(!free.start_solid);
    }

    #[test]
    fn real_map_tree_traces_match_legacy_brush_scan() {
        let basewsw = std::env::var("WARSOW_BASEWSW")
            .unwrap_or_else(|_| r"C:\Program Files (x86)\Warsow 2.1\basewsw".into());
        let pk3 = std::path::Path::new(&basewsw).join("map_wbomb1.pk3");
        if !pk3.exists() {
            eprintln!(
                "SKIPPED: {} not found; real collision trace NOT tested",
                pk3.display()
            );
            return;
        }
        let data = super::super::read_pk3_entry(&pk3, "maps/wbomb1.bsp").unwrap();
        let bsp = Bsp::parse(&data).unwrap();
        let model = &bsp.models[0];
        let center: [f32; 3] =
            std::array::from_fn(|axis| (model.mins[axis] + model.maxs[axis]) * 0.5);
        let traces = [
            (
                [center[0], center[1], model.maxs[2] + 128.0],
                [center[0], center[1], model.mins[2] - 128.0],
                [-16.0, -16.0, -24.0],
                [16.0, 16.0, 40.0],
            ),
            (
                [model.mins[0] - 128.0, center[1], center[2]],
                [model.maxs[0] + 128.0, center[1], center[2]],
                [0.0; 3],
                [0.0; 3],
            ),
            (
                [center[0], model.mins[1] - 128.0, center[2]],
                [center[0], model.maxs[1] + 128.0, center[2]],
                [-8.0; 3],
                [8.0; 3],
            ),
        ];
        for (start, end, mins, maxs) in traces {
            let tree = bsp.trace(start, end, mins, maxs, 1).unwrap();
            let scan = trace_all_leaf_brushes(&bsp, start, end, mins, maxs, 1);
            assert!((tree.fraction - scan.fraction).abs() <= 1e-5);
            assert!(
                tree.end_position
                    .iter()
                    .zip(scan.end_position)
                    .all(|(a, b)| (a - b).abs() <= 0.05)
            );
            assert_eq!(
                (tree.start_solid, tree.all_solid),
                (scan.start_solid, scan.all_solid)
            );
            assert_eq!(tree.contents, scan.contents);
        }
    }
}
