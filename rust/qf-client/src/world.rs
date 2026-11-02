//! Textured world drawing: turns the parsed BSP, shader scripts and VFS content into GPU
//! batches and draws them stage by stage (legacy `R_DrawSurfaces` semantics, simplified).

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use qf_engine::content::bsp::{Bsp, WorldMesh};
use qf_engine::content::image::{Rgba8Image, load_image};
use qf_engine::content::shader::{
    AlphaFunc, AlphaGen, Blend, BlendFactor, MapSource, RgbGen, Shader, ShaderLib, Stage, TcGen,
    TcMod,
};
use qf_engine::content::vfs::Vfs;
use qf_engine::simulation::entities::MoverPose;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const SURF_SKY: i32 = 0x4;
const SURF_NODRAW: i32 = 0x80;
const SURF_SKIP: i32 = 0x200;
const MAX_STAGES: usize = 8;
/// Multiplier applied to lightmap and vertex-lit colours (legacy overbright).
const LIGHT_SCALE: f32 = 1.0;
/// Half extent of the skybox cube, inside the far plane.
const SKY_SIZE: f32 = 4096.0;

/// Skybox faces (image suffix, corners at u0v0, u1v0, u1v1, u0v1) in Quake axes.
const SKY_FACES: [(&str, [[f32; 3]; 4]); 6] = [
    (
        "ft",
        [[1., 1., 1.], [1., -1., 1.], [1., -1., -1.], [1., 1., -1.]],
    ),
    (
        "bk",
        [
            [-1., -1., 1.],
            [-1., 1., 1.],
            [-1., 1., -1.],
            [-1., -1., -1.],
        ],
    ),
    (
        "lf",
        [[-1., 1., 1.], [1., 1., 1.], [1., 1., -1.], [-1., 1., -1.]],
    ),
    (
        "rt",
        [
            [1., -1., 1.],
            [-1., -1., 1.],
            [-1., -1., -1.],
            [1., -1., -1.],
        ],
    ),
    (
        "up",
        [[1., 1., 1.], [-1., 1., 1.], [-1., -1., 1.], [1., -1., 1.]],
    ),
    (
        "dn",
        [
            [1., -1., -1.],
            [-1., -1., -1.],
            [-1., 1., -1.],
            [1., 1., -1.],
        ],
    ),
];
const SKY_UVS: [[f32; 2]; 4] = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];

struct Sky {
    shaders: [Option<usize>; 6],
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
}

/// Game content shared by the world renderer: file system and parsed shader scripts.
pub struct Content {
    pub vfs: Vfs,
    pub shaders: ShaderLib,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    lm_uv: [f32; 2],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    mvp: [[f32; 4]; 4],
    eye_time: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct StageUniform {
    tc0: [f32; 4],
    tc1: [f32; 4],
    vec_s: [f32; 4],
    vec_t: [f32; 4],
    color: [f32; 4],
    modes: [u32; 4],
    test: [u32; 4],
    turb: [f32; 4],
}

fn build_vertices(mesh: &WorldMesh) -> Vec<Vertex> {
    (0..mesh.positions.len())
        .map(|i| Vertex {
            position: mesh.positions[i],
            normal: mesh.normals[i],
            uv: mesh.uvs[i],
            lm_uv: mesh.lm_uvs[i],
            color: mesh.colors[i],
        })
        .collect()
}

fn check_mesh(mesh: &WorldMesh) -> Result<(), String> {
    let n = mesh.positions.len();
    if mesh.normals.len() != n
        || mesh.uvs.len() != n
        || mesh.lm_uvs.len() != n
        || mesh.colors.len() != n
        || !mesh.indices.len().is_multiple_of(3)
    {
        return Err("parsed BSP mesh has inconsistent vertex or index data".into());
    }
    Ok(())
}

/// Row-major 2x3 texture matrix `[m00 m01 m10 m11 t0 t1]`: `s' = s*m00 + t*m10 + t0`.
type Affine = [f32; 6];

const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn then(a: Affine, b: Affine) -> Affine {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn tc_affine(mods: &[TcMod], time: f32) -> Affine {
    let mut m = IDENTITY;
    for tc in mods {
        let step = match tc {
            TcMod::Scroll(x, y) => [
                1.0,
                0.0,
                0.0,
                1.0,
                (x * time).rem_euclid(1.0),
                (y * time).rem_euclid(1.0),
            ],
            TcMod::Scale(x, y) => [*x, 0.0, 0.0, *y, 0.0, 0.0],
            TcMod::Rotate(deg_per_sec) => {
                let (s, c) = (-deg_per_sec * time).to_radians().sin_cos();
                [
                    c,
                    s,
                    -s,
                    c,
                    0.5 - 0.5 * c + 0.5 * s,
                    0.5 - 0.5 * s - 0.5 * c,
                ]
            }
            TcMod::Stretch(w) => {
                let v = w.eval(time);
                let p = if v.abs() > 1e-4 { 1.0 / v } else { 1.0 };
                [p, 0.0, 0.0, p, 0.5 - 0.5 * p, 0.5 - 0.5 * p]
            }
            TcMod::Transform(t) => *t,
            TcMod::Turb { .. } => continue,
        };
        m = then(m, step);
    }
    m
}

fn stage_uniform(stage: &Stage, time: f32) -> StageUniform {
    let mut u = StageUniform {
        color: [1.0; 4],
        ..Default::default()
    };
    u.modes[0] = match stage.map {
        MapSource::Lightmap => 1,
        MapSource::White => 2,
        _ => 0,
    };
    match &stage.rgb_gen {
        RgbGen::Vertex | RgbGen::ExactVertex => u.modes[1] = 1,
        RgbGen::OneMinusVertex => u.modes[1] = 2,
        RgbGen::Const(c) => u.color[..3].copy_from_slice(c),
        RgbGen::Wave(w) => {
            let v = w.eval(time).clamp(0.0, 1.0);
            u.color[..3].copy_from_slice(&[v; 3]);
        }
        _ => {}
    }
    match &stage.alpha_gen {
        AlphaGen::Vertex => u.modes[2] = 1,
        AlphaGen::Const(a) => u.color[3] = *a,
        AlphaGen::Wave(w) => u.color[3] = w.eval(time).clamp(0.0, 1.0),
        AlphaGen::Identity => {}
    }
    match &stage.tc_gen {
        TcGen::Base => {}
        TcGen::Lightmap => u.modes[3] = 1,
        TcGen::Environment => u.modes[3] = 2,
        TcGen::Vector(s, t) => {
            u.modes[3] = 3;
            u.vec_s[..3].copy_from_slice(s);
            u.vec_t[..3].copy_from_slice(t);
        }
    }
    u.test[0] = match stage.alpha_func {
        None => 0,
        Some(AlphaFunc::Gt0) => 1,
        Some(AlphaFunc::Lt128) => 2,
        Some(AlphaFunc::Ge128) => 3,
    };
    let m = tc_affine(&stage.tc_mods, time);
    u.tc0 = [m[0], m[2], m[4], 0.0];
    u.tc1 = [m[1], m[3], m[5], 0.0];
    for tc in &stage.tc_mods {
        if let TcMod::Turb {
            amp, phase, freq, ..
        } = tc
        {
            u.turb = [*amp, phase + time * freq, 0.0, 0.0];
        }
    }
    u
}

fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    match f {
        BlendFactor::Zero => wgpu::BlendFactor::Zero,
        BlendFactor::One => wgpu::BlendFactor::One,
        BlendFactor::SrcColor => wgpu::BlendFactor::Src,
        BlendFactor::OneMinusSrcColor => wgpu::BlendFactor::OneMinusSrc,
        BlendFactor::DstColor => wgpu::BlendFactor::Dst,
        BlendFactor::OneMinusDstColor => wgpu::BlendFactor::OneMinusDst,
        BlendFactor::SrcAlpha => wgpu::BlendFactor::SrcAlpha,
        BlendFactor::OneMinusSrcAlpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        BlendFactor::DstAlpha => wgpu::BlendFactor::DstAlpha,
        BlendFactor::OneMinusDstAlpha => wgpu::BlendFactor::OneMinusDstAlpha,
    }
}

type PipelineKey = (Option<(wgpu::BlendFactor, wgpu::BlendFactor)>, bool, bool);

struct StageDraw {
    slot: u32,
    pipeline: usize,
    /// One bind group per animation frame.
    frames: Vec<wgpu::BindGroup>,
    fps: f32,
    def: Stage,
}

struct PreparedShader {
    translucent: bool,
    stages: Vec<StageDraw>,
}

struct Batch {
    shader: usize,
    first: u32,
    count: u32,
}

struct MoverModel {
    model: usize,
    vertices: Vec<Vertex>,
    batches: Vec<Batch>,
}

pub struct WorldRenderer {
    pipelines: Vec<wgpu::RenderPipeline>,
    shaders: Vec<PreparedShader>,
    view_buf: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    stage_buf: wgpu::Buffer,
    stage_stride: u64,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    batches: Vec<Batch>,
    movers: Vec<MoverModel>,
    mover_vertex_buf: wgpu::Buffer,
    mover_index_buf: wgpu::Buffer,
    mover_capacity: usize,
    mover_vertices: Vec<Vertex>,
    mover_draws: Vec<(usize, u32)>,
    time: f32,
    sky: Option<Sky>,
}

struct Builder<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    content: &'a Content,
    bsp: &'a Bsp,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    stage_bgl: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    repeat: wgpu::Sampler,
    clamp: wgpu::Sampler,
    white: wgpu::TextureView,
    stage_buf: &'a wgpu::Buffer,
    textures: HashMap<String, wgpu::TextureView>,
    pipelines: Vec<wgpu::RenderPipeline>,
    pipeline_ids: HashMap<PipelineKey, usize>,
    shaders: Vec<PreparedShader>,
    by_ref: HashMap<u32, Option<usize>>,
    next_slot: u32,
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    image: &Rgba8Image,
) -> wgpu::TextureView {
    let mips = image.mip_chain();
    let data: Vec<u8> = mips.iter().flat_map(|m| m.pixels.iter().copied()).collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: image.width,
                    height: image.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: mips.len() as u32,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        )
        .create_view(&Default::default())
}

impl Builder<'_> {
    fn texture(&mut self, name: &str) -> wgpu::TextureView {
        let key = name.to_ascii_lowercase();
        if let Some(v) = self.textures.get(&key) {
            return v.clone();
        }
        let image = match load_image(&self.content.vfs, name) {
            Ok(image) => image,
            Err(e) => {
                eprintln!("texture: '{name}' missing or unreadable ({e:?}); using checker");
                Rgba8Image::checker(64, 8)
            }
        };
        let view = upload_texture(self.device, self.queue, name, &image);
        self.textures.insert(key, view.clone());
        view
    }

    fn pipeline(&mut self, stage: &Stage) -> usize {
        let blend = stage
            .blend
            .map(|Blend { src, dst }| (blend_factor(src), blend_factor(dst)));
        let key = (blend, stage.depth_write, stage.depth_equal);
        if let Some(&id) = self.pipeline_ids.get(&key) {
            return id;
        }
        let component = |(src_factor, dst_factor)| wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("BSP stage"),
                layout: Some(&self.layout),
                vertex: wgpu::VertexState {
                    module: &self.module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2,
                            4 => Unorm8x4
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        blend: blend.map(|b| wgpu::BlendState {
                            color: component(b),
                            alpha: component(b),
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Surface winding is not trusted yet, so every surface is double sided.
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(stage.depth_write),
                    depth_compare: Some(if stage.depth_equal {
                        wgpu::CompareFunction::Equal
                    } else {
                        wgpu::CompareFunction::LessEqual
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            });
        self.pipelines.push(pipeline);
        let id = self.pipelines.len() - 1;
        self.pipeline_ids.insert(key, id);
        id
    }

    /// Prepared shader for a BSP shader reference; `None` when the surface is not drawn
    /// by the world pass (nodraw/skip/sky or no stages).
    fn shader_for_ref(&mut self, r: u32) -> Option<usize> {
        if let Some(&cached) = self.by_ref.get(&r) {
            return cached;
        }
        let prepared = self.prepare_ref(r);
        self.by_ref.insert(r, prepared);
        prepared
    }

    fn prepare_ref(&mut self, r: u32) -> Option<usize> {
        let (name, flags) = match self.bsp.shaders.get(r as usize) {
            Some(s) => (s.name.clone(), s.surface_flags),
            None => {
                eprintln!("shader: surface references missing shader #{r}");
                (format!("*missing-shader-{r}"), 0)
            }
        };
        let shader: Shader = self.content.shaders.resolve(&name);
        if flags & (SURF_NODRAW | SURF_SKIP) != 0 || shader.is_hidden() {
            return None;
        }
        if flags & SURF_SKY != 0 || shader.is_sky() {
            return None;
        }
        self.build_shader(&shader.stages, shader.is_translucent(), &name)
    }

    /// Uploads textures and bind groups for `defs`; `None` when there is nothing to draw.
    fn build_shader(&mut self, defs: &[Stage], translucent: bool, name: &str) -> Option<usize> {
        if defs.len() > MAX_STAGES {
            eprintln!("shader: '{name}' has more than {MAX_STAGES} stages; extra stages ignored");
        }
        let mut stages = Vec::new();
        for def in defs.iter().take(MAX_STAGES) {
            let names: Vec<String> = match &def.map {
                MapSource::Image(n) => vec![n.clone()],
                MapSource::Anim { frames, .. } => frames.clone(),
                _ => Vec::new(),
            };
            let views: Vec<wgpu::TextureView> = if names.is_empty() {
                vec![self.white.clone()]
            } else {
                names.iter().map(|n| self.texture(n)).collect()
            };
            let sampler = if def.clamp { &self.clamp } else { &self.repeat };
            let frames = views
                .iter()
                .map(|view| {
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("BSP stage"),
                        layout: &self.stage_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: self.stage_buf,
                                    offset: 0,
                                    size: NonZeroU64::new(
                                        std::mem::size_of::<StageUniform>() as u64
                                    ),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(sampler),
                            },
                        ],
                    })
                })
                .collect();
            let fps = match &def.map {
                MapSource::Anim { fps, .. } => *fps,
                _ => 0.0,
            };
            let pipeline = self.pipeline(def);
            stages.push(StageDraw {
                slot: self.next_slot,
                pipeline,
                frames,
                fps,
                def: def.clone(),
            });
            self.next_slot += 1;
        }
        if stages.is_empty() {
            return None;
        }
        self.shaders.push(PreparedShader {
            translucent,
            stages,
        });
        Some(self.shaders.len() - 1)
    }

    /// First skybox declared by a sky surface of the map, one shader per cube face.
    fn skybox(&mut self) -> Option<[Option<usize>; 6]> {
        let far_box = (0..self.bsp.shaders.len()).find_map(|r| {
            let s = &self.bsp.shaders[r];
            let shader = self.content.shaders.resolve(&s.name);
            if s.surface_flags & SURF_SKY == 0 && !shader.is_sky() {
                return None;
            }
            shader.sky.and_then(|p| p.far_box)
        })?;
        Some(SKY_FACES.map(|(suffix, ..)| {
            let mut stage = Stage::new(MapSource::Image(format!("{far_box}_{suffix}")));
            stage.clamp = true;
            stage.depth_write = false;
            self.build_shader(&[stage], false, &far_box)
        }))
    }

    /// Groups the mesh triangles by shader and appends them to `indices`.
    fn batch_mesh(&mut self, mesh: &WorldMesh, indices: &mut Vec<u32>) -> Vec<Batch> {
        let mut by_ref: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for (t, tri) in mesh.indices.chunks_exact(3).enumerate() {
            let r = mesh.triangle_shaders.get(t).copied().unwrap_or(u32::MAX);
            by_ref.entry(r).or_default().extend_from_slice(tri);
        }
        let mut batches = Vec::new();
        for (r, tris) in by_ref {
            let Some(shader) = self.shader_for_ref(r) else {
                continue;
            };
            if std::env::var_os("QF_DEBUG_SHADERS").is_some() {
                let name = self.bsp.shaders.get(r as usize).map_or("?", |s| &s.name);
                eprintln!("batch {name}: {} triangles", tris.len() / 3);
            }
            batches.push(Batch {
                shader,
                first: indices.len() as u32,
                count: tris.len() as u32,
            });
            indices.extend(tris);
        }
        batches
    }
}

impl WorldRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        content: &Content,
        bsp: &Bsp,
        world: &WorldMesh,
        submodels: &[(usize, WorldMesh)],
    ) -> Result<Self, String> {
        check_mesh(world)?;
        for (_, m) in submodels {
            check_mesh(m)?;
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BSP world"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bsp.wgsl").into()),
        });
        let uniform_entry = |binding, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let view_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BSP view"),
            entries: &[uniform_entry(0, false), texture_entry(1), sampler_entry(2)],
        });
        let stage_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BSP stage"),
            entries: &[uniform_entry(0, true), texture_entry(1), sampler_entry(2)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("BSP world"),
            bind_group_layouts: &[Some(&view_bgl), Some(&stage_bgl)],
            immediate_size: 0,
        });
        let sampler = |address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: address,
                address_mode_v: address,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp: 8,
                ..Default::default()
            })
        };
        let repeat = sampler(wgpu::AddressMode::Repeat);
        let clamp = sampler(wgpu::AddressMode::ClampToEdge);
        let white = upload_texture(device, queue, "white", &Rgba8Image::solid([255; 4]));

        let stage_stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(std::mem::size_of::<StageUniform>() as u64);
        let slot_capacity = (bsp.shaders.len().max(1) * MAX_STAGES + MAX_STAGES) as u64;
        let stage_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP stage uniforms"),
            size: slot_capacity * stage_stride,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut builder = Builder {
            device,
            queue,
            content,
            bsp,
            module,
            layout,
            stage_bgl,
            format,
            repeat,
            clamp,
            white: white.clone(),
            stage_buf: &stage_buf,
            textures: HashMap::new(),
            pipelines: Vec::new(),
            pipeline_ids: HashMap::new(),
            shaders: Vec::new(),
            by_ref: HashMap::new(),
            next_slot: 0,
        };

        let mut world_indices = Vec::new();
        let batches = builder.batch_mesh(world, &mut world_indices);
        let mut mover_indices = Vec::new();
        let mut movers = Vec::new();
        for (model, mesh) in submodels {
            movers.push(MoverModel {
                model: *model,
                vertices: build_vertices(mesh),
                batches: builder.batch_mesh(mesh, &mut mover_indices),
            });
        }
        let sky_shaders = builder.skybox();
        let Builder {
            pipelines, shaders, ..
        } = builder;
        let sky = sky_shaders.map(|shaders| Sky {
            shaders,
            vertex_buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Skybox vertices"),
                size: (24 * std::mem::size_of::<Vertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            index_buf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Skybox indices"),
                contents: bytemuck::cast_slice(
                    &(0..6u32)
                        .flat_map(|f| [0, 1, 2, 0, 2, 3].map(|i| f * 4 + i))
                        .collect::<Vec<_>>(),
                ),
                usage: wgpu::BufferUsages::INDEX,
            }),
        });

        let lightmap = lightmap_view(device, queue, bsp, &white);
        let lm_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let view_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP view uniforms"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BSP view"),
            layout: &view_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: view_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&lightmap),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&lm_sampler),
                },
            ],
        });

        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP world vertices"),
            contents: bytemuck::cast_slice(&build_vertices(world)),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP world indices"),
            contents: bytemuck::cast_slice(&world_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let mover_capacity: usize = movers.iter().map(|m| m.vertices.len()).sum();
        let mover_vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP mover vertices"),
            size: (mover_capacity.max(1) * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mover_index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP mover indices"),
            contents: &if mover_indices.is_empty() {
                vec![0u8; 4]
            } else {
                bytemuck::cast_slice(&mover_indices).to_vec()
            },
            usage: wgpu::BufferUsages::INDEX,
        });

        Ok(Self {
            pipelines,
            shaders,
            view_buf,
            view_bg,
            stage_buf,
            stage_stride,
            vertex_buf,
            index_buf,
            batches,
            movers,
            mover_vertex_buf,
            mover_index_buf,
            mover_capacity,
            mover_vertices: Vec::new(),
            mover_draws: Vec::new(),
            time: 0.0,
            sky,
        })
    }

    /// Uploads per-frame data: view, shader-stage parameters at `time` and posed movers.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        mvp: [[f32; 4]; 4],
        eye: [f32; 3],
        time: f32,
        movers: &[MoverPose],
    ) {
        self.time = time;
        queue.write_buffer(
            &self.view_buf,
            0,
            bytemuck::bytes_of(&ViewUniform {
                mvp,
                eye_time: [eye[0], eye[1], eye[2], time],
                params: [LIGHT_SCALE, 0.0, 0.0, 0.0],
            }),
        );

        let slots = self
            .shaders
            .iter()
            .flat_map(|s| &s.stages)
            .map(|s| s.slot as usize + 1)
            .max()
            .unwrap_or(0);
        let stride = self.stage_stride as usize;
        let mut bytes = vec![0u8; slots * stride];
        for stage in self.shaders.iter().flat_map(|s| &s.stages) {
            let u = stage_uniform(&stage.def, time);
            let at = stage.slot as usize * stride;
            bytes[at..at + std::mem::size_of::<StageUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.stage_buf, 0, &bytes);
        }

        self.mover_vertices.clear();
        self.mover_draws.clear();
        if let Some(sky) = &self.sky {
            let verts: Vec<Vertex> = SKY_FACES
                .iter()
                .flat_map(|(_, corners)| {
                    corners.iter().zip(SKY_UVS).map(|(c, uv)| Vertex {
                        position: std::array::from_fn(|i| eye[i] + c[i] * SKY_SIZE),
                        normal: [0.0; 3],
                        uv,
                        lm_uv: [-1.0, -1.0],
                        color: [255; 4],
                    })
                })
                .collect();
            queue.write_buffer(&sky.vertex_buf, 0, bytemuck::cast_slice(&verts));
        }
        for pose in movers {
            let Some(idx) = self.movers.iter().position(|m| m.model == pose.model) else {
                continue;
            };
            let model = &self.movers[idx];
            if self.mover_vertices.len() + model.vertices.len() > self.mover_capacity {
                continue;
            }
            self.mover_draws
                .push((idx, self.mover_vertices.len() as u32));
            for v in &model.vertices {
                let rotated = pose.local_to_world(v.normal);
                self.mover_vertices.push(Vertex {
                    position: pose.local_to_world(v.position),
                    normal: std::array::from_fn(|i| rotated[i] - pose.origin[i]),
                    ..*v
                });
            }
        }
        if !self.mover_vertices.is_empty() {
            queue.write_buffer(
                &self.mover_vertex_buf,
                0,
                bytemuck::cast_slice(&self.mover_vertices),
            );
        }
    }

    fn draw_batch(&self, pass: &mut wgpu::RenderPass<'_>, batch: &Batch, base_vertex: i32) {
        let shader = &self.shaders[batch.shader];
        for stage in &shader.stages {
            let frame = if stage.frames.len() > 1 {
                (self.time * stage.fps).max(0.0) as usize % stage.frames.len()
            } else {
                0
            };
            pass.set_pipeline(&self.pipelines[stage.pipeline]);
            pass.set_bind_group(
                1,
                &stage.frames[frame],
                &[(u64::from(stage.slot) * self.stage_stride) as u32],
            );
            pass.draw_indexed(batch.first..batch.first + batch.count, base_vertex, 0..1);
        }
    }

    /// Draws opaque world, opaque movers, then translucent world and movers.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.view_bg, &[]);
        if let Some(sky) = &self.sky {
            pass.set_vertex_buffer(0, sky.vertex_buf.slice(..));
            pass.set_index_buffer(sky.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            for (face, shader) in sky.shaders.iter().enumerate() {
                if let Some(shader) = shader {
                    let batch = Batch {
                        shader: *shader,
                        first: face as u32 * 6,
                        count: 6,
                    };
                    self.draw_batch(pass, &batch, 0);
                }
            }
        }
        for translucent in [false, true] {
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            for b in self
                .batches
                .iter()
                .filter(|b| self.shaders[b.shader].translucent == translucent)
            {
                self.draw_batch(pass, b, 0);
            }
            if self.mover_draws.is_empty() {
                continue;
            }
            pass.set_vertex_buffer(0, self.mover_vertex_buf.slice(..));
            pass.set_index_buffer(self.mover_index_buf.slice(..), wgpu::IndexFormat::Uint32);
            for &(idx, base) in &self.mover_draws {
                for b in self.movers[idx]
                    .batches
                    .iter()
                    .filter(|b| self.shaders[b.shader].translucent == translucent)
                {
                    self.draw_batch(pass, b, base as i32);
                }
            }
        }
    }
}

fn lightmap_view(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bsp: &Bsp,
    white: &wgpu::TextureView,
) -> wgpu::TextureView {
    let atlas = &bsp.lightmaps;
    if atlas.rgba.is_empty() || atlas.width == 0 || atlas.height == 0 {
        eprintln!("lightmap: map has no usable lightmaps; surfaces use vertex colours");
        return white.clone();
    }
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("lightmap atlas"),
                size: wgpu::Extent3d {
                    width: atlas.width,
                    height: atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &atlas.rgba,
        )
        .create_view(&Default::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_and_scale_compose_in_order() {
        let m = tc_affine(&[TcMod::Scale(2.0, 2.0), TcMod::Scroll(0.5, 0.0)], 1.0);
        assert_eq!(m, [2.0, 0.0, 0.0, 2.0, 0.5, 0.0]);
    }

    #[test]
    fn rotate_keeps_center_fixed() {
        let m = tc_affine(&[TcMod::Rotate(90.0)], 1.0);
        let (s, t) = (0.5f32, 0.5f32);
        let s2 = s * m[0] + t * m[2] + m[4];
        let t2 = s * m[1] + t * m[3] + m[5];
        assert!((s2 - 0.5).abs() < 1e-5 && (t2 - 0.5).abs() < 1e-5);
    }
}
