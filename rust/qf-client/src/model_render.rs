//! Static md3 props (items, weapons, flags): render-only, driven by placements and time.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use qf_engine::content::bsp::entities::Entity;
use qf_engine::content::bsp::lightgrid::{GridLight, LightGrid};
use qf_engine::content::image::{Rgba8Image, load_mip_chain};
use qf_engine::content::md3::{self, Md3Surface};
use qf_engine::content::shader::{Blend, MapSource, Shader};

use crate::dlight::{self, DynamicLight, PackedLights};
use crate::item_models::model_for_classname;
use crate::math::Mat4;
use crate::world::{Content, DEPTH_FORMAT, blend_factor, upload_texture};

const SPIN_DEG_PER_SEC: f32 = 90.0;
const BOB_AMPLITUDE: f32 = 3.0;
const BOB_HZ: f32 = 0.5;
/// Items rest on the floor; sampling slightly above avoids the cell below the floor.
const LIGHT_SAMPLE_HEIGHT: f32 = 16.0;

/// A prop to draw: an md3 at a position.
#[derive(Clone, Debug)]
pub struct Prop {
    pub model: String,
    /// Position of the entity in the map's entity lump (usize::MAX when not from a lump).
    pub entity: usize,
    pub origin: [f32; 3],
    pub yaw_degrees: f32,
    /// Items spin and bob; flags and indicators stand still.
    pub animated: bool,
    /// Baked light at the prop, from the map's light grid.
    pub light: GridLight,
}

impl Prop {
    pub fn for_classname(classname: &str, origin: [f32; 3], yaw_degrees: f32) -> Option<Self> {
        let model = model_for_classname(classname)?;
        let lower = classname.to_ascii_lowercase();
        let animated = ['w', 'a', 'i'].iter().any(|&p| lower.starts_with(p));
        Some(Self {
            model,
            entity: usize::MAX,
            origin,
            yaw_degrees,
            animated,
            light: GridLight::default(),
        })
    }
}

/// Props for every entity of a map lump that has a world model.
pub fn props_from_entities(lump: &[Entity], grid: &LightGrid) -> Vec<Prop> {
    lump.iter()
        .enumerate()
        .filter_map(|(entity, e)| {
            let origin = parse_floats::<3>(e.get("origin")?)?;
            let yaw = e
                .get("angle")
                .and_then(|a| a.trim().parse::<f32>().ok())
                .unwrap_or(0.0);
            let mut prop = Prop::for_classname(e.get("classname")?, origin, yaw)?;
            prop.entity = entity;
            prop.light = grid.sample([origin[0], origin[1], origin[2] + LIGHT_SAMPLE_HEIGHT]);
            Some(prop)
        })
        .collect()
}

fn point4(v: [f32; 3]) -> [f32; 4] {
    [v[0], v[1], v[2], 0.0]
}

fn parse_floats<const N: usize>(text: &str) -> Option<[f32; N]> {
    let v: Vec<f32> = text
        .split_ascii_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let a: [f32; N] = v.try_into().ok()?;
    a.iter().all(|c| c.is_finite()).then_some(a)
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ModelVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ViewUniform {
    mvp: Mat4,
    eye: [f32; 4],
    light_count: [f32; 4],
    lights: PackedLights,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct InstanceUniform {
    model: Mat4,
    tint: [f32; 4],
    light_dir: [f32; 4],
    ambient: [f32; 4],
    diffuse: [f32; 4],
}

#[derive(Clone, Copy, PartialEq)]
struct PipelineKey {
    blend: Option<Blend>,
    alpha_test: bool,
    depth_write: bool,
}

struct Surface {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    texture_bg: wgpu::BindGroup,
    pipeline: usize,
    blended: bool,
}

struct Model {
    surfaces: Vec<Surface>,
}

pub struct ModelRenderer {
    pipelines: Vec<(PipelineKey, wgpu::RenderPipeline)>,
    view_buf: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
    instance_bg: wgpu::BindGroup,
    instance_stride: u64,
    models: Vec<Model>,
    /// Per prop: index into `models`, if its model loaded.
    props: Vec<(Option<usize>, Prop)>,
    visible: Vec<bool>,
}

fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if !name[dot..].contains('/') => &name[..dot],
        _ => name,
    }
}

/// Diffuse image and blending of the first image stage of a surface shader.
fn surface_look(shader: &Shader, fallback: &str) -> (String, Option<Blend>, bool) {
    for stage in &shader.stages {
        let image = match &stage.map {
            MapSource::Image(name) => name.clone(),
            MapSource::Anim { frames, .. } if !frames.is_empty() => frames[0].clone(),
            _ => continue,
        };
        // Lightmap-modulate blends (`dst_color zero`) are plain opaque on a model.
        let blend = stage.blend.filter(|b| {
            !(b.src == qf_engine::content::shader::BlendFactor::DstColor
                && b.dst == qf_engine::content::shader::BlendFactor::Zero)
        });
        return (image, blend, stage.alpha_func.is_some());
    }
    (fallback.to_string(), None, false)
}

impl ModelRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        content: &Content,
        props: Vec<Prop>,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("md3 props"),
            source: wgpu::ShaderSource::Wgsl(include_str!("model.wgsl").into()),
        });
        let uniform_bgl = |dynamic, label| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: dynamic,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        };
        let view_bgl = uniform_bgl(false, "md3 view");
        let instance_bgl = uniform_bgl(true, "md3 instance");
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("md3 texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("md3 props"),
            bind_group_layouts: &[Some(&view_bgl), Some(&instance_bgl), Some(&texture_bgl)],
            immediate_size: 0,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let view_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("md3 view"),
            size: std::mem::size_of::<ViewUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("md3 view"),
            layout: &view_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: view_buf.as_entire_binding(),
            }],
        });
        let instance_stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(std::mem::size_of::<InstanceUniform>() as u64);
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("md3 instances"),
            size: instance_stride * props.len().max(1) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("md3 instance"),
            layout: &instance_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &instance_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<InstanceUniform>() as u64),
                }),
            }],
        });

        let mut pipelines: Vec<(PipelineKey, wgpu::RenderPipeline)> = Vec::new();
        let mut textures: HashMap<String, wgpu::TextureView> = HashMap::new();
        let mut model_ids: HashMap<String, Option<usize>> = HashMap::new();
        let mut models: Vec<Model> = Vec::new();
        let mut placed = Vec::with_capacity(props.len());

        for prop in props {
            let key = prop.model.to_ascii_lowercase();
            let id = *model_ids.entry(key).or_insert_with(|| {
                let data = match content.vfs.read(&prop.model) {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("model: '{}' unreadable ({e:?})", prop.model);
                        return None;
                    }
                };
                let parsed = match md3::parse(&data) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("model: '{}': {e}", prop.model);
                        return None;
                    }
                };
                let mut surfaces = Vec::new();
                for s in &parsed.surfaces {
                    surfaces.push(build_surface(
                        device,
                        queue,
                        format,
                        &module,
                        &layout,
                        &texture_bgl,
                        &sampler,
                        content,
                        &mut textures,
                        &mut pipelines,
                        s,
                    ));
                }
                models.push(Model { surfaces });
                Some(models.len() - 1)
            });
            placed.push((id, prop));
        }

        let visible = vec![true; placed.len()];
        Self {
            pipelines,
            view_buf,
            view_bg,
            instance_buf,
            instance_bg,
            instance_stride,
            models,
            props: placed,
            visible,
        }
    }

    /// Hides or shows the prop of a map entity (e.g. a taken pickup).
    pub fn set_entity_visible(&mut self, entity: usize, visible: bool) {
        for (i, (_, prop)) in self.props.iter().enumerate() {
            if prop.entity == entity {
                self.visible[i] = visible;
            }
        }
    }

    pub fn prepare(
        &self,
        queue: &wgpu::Queue,
        mvp: Mat4,
        eye: [f32; 3],
        time: f32,
        lights: &[DynamicLight],
    ) {
        let (lights, count) = dlight::pack(lights);
        queue.write_buffer(
            &self.view_buf,
            0,
            bytemuck::bytes_of(&ViewUniform {
                mvp,
                eye: [eye[0], eye[1], eye[2], time],
                light_count: [count as f32, 0.0, 0.0, 0.0],
                lights,
            }),
        );
        let stride = self.instance_stride as usize;
        let mut bytes = vec![0u8; self.props.len() * stride];
        for (i, (_, prop)) in self.props.iter().enumerate() {
            let u = InstanceUniform {
                model: prop_matrix(prop, time),
                tint: [1.0; 4],
                light_dir: point4(prop.light.direction),
                ambient: point4(prop.light.ambient),
                diffuse: point4(prop.light.diffuse),
            };
            let at = i * stride;
            bytes[at..at + std::mem::size_of::<InstanceUniform>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.instance_buf, 0, &bytes);
        }
    }

    /// Opaque surfaces first, then blended ones.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_bind_group(0, &self.view_bg, &[]);
        for blended in [false, true] {
            for (i, (model, _)) in self.props.iter().enumerate() {
                let Some(model) = model.map(|m| &self.models[m]) else {
                    continue;
                };
                if !self.visible[i] {
                    continue;
                }
                let offset = (i as u64 * self.instance_stride) as u32;
                pass.set_bind_group(1, &self.instance_bg, &[offset]);
                for s in model.surfaces.iter().filter(|s| s.blended == blended) {
                    pass.set_pipeline(&self.pipelines[s.pipeline].1);
                    pass.set_bind_group(2, &s.texture_bg, &[]);
                    pass.set_vertex_buffer(0, s.vertex_buf.slice(..));
                    pass.set_index_buffer(s.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..s.index_count, 0, 0..1);
                }
            }
        }
    }
}

fn prop_matrix(prop: &Prop, time: f32) -> Mat4 {
    let (yaw, bob) = if prop.animated {
        (
            prop.yaw_degrees + time * SPIN_DEG_PER_SEC,
            (time * BOB_HZ * std::f32::consts::TAU).sin() * BOB_AMPLITUDE,
        )
    } else {
        (prop.yaw_degrees, 0.0)
    };
    let (s, c) = yaw.to_radians().sin_cos();
    let [x, y, z] = prop.origin;
    [
        [c, s, 0.0, 0.0],
        [-s, c, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [x, y, z + bob, 1.0],
    ]
}

#[allow(clippy::too_many_arguments)]
fn build_surface(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    texture_bgl: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    content: &Content,
    textures: &mut HashMap<String, wgpu::TextureView>,
    pipelines: &mut Vec<(PipelineKey, wgpu::RenderPipeline)>,
    surface: &Md3Surface,
) -> Surface {
    let shader_name = surface.shaders.first().map_or("", |s| strip_extension(s));
    let (image, blend, alpha_test) =
        surface_look(&content.shaders.resolve(shader_name), shader_name);
    let view = textures
        .entry(image.to_ascii_lowercase())
        .or_insert_with(|| {
            let mips = load_mip_chain(&content.vfs, &image).unwrap_or_else(|e| {
                eprintln!("texture: '{image}' missing or unreadable ({e:?}); using checker");
                Rgba8Image::checker(64, 8).mip_chain()
            });
            upload_texture(device, queue, &image, &mips)
        })
        .clone();
    let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("md3 texture"),
        layout: texture_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });

    let verts: Vec<ModelVertex> = surface
        .frame_positions(0)
        .iter()
        .zip(surface.frame_normals(0))
        .zip(&surface.uvs)
        .map(|((&position, &normal), &uv)| ModelVertex {
            position,
            normal,
            uv,
        })
        .collect();
    let indices: Vec<u32> = surface.triangles.iter().flatten().copied().collect();
    let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("md3 vertices"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("md3 indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    });

    let key = PipelineKey {
        blend,
        alpha_test,
        depth_write: blend.is_none(),
    };
    let pipeline = match pipelines.iter().position(|(k, _)| *k == key) {
        Some(i) => i,
        None => {
            pipelines.push((key, make_pipeline(device, format, module, layout, key)));
            pipelines.len() - 1
        }
    };
    Surface {
        vertex_buf,
        index_buf,
        index_count: indices.len() as u32,
        texture_bg,
        pipeline,
        blended: blend.is_some(),
    }
}

fn make_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    key: PipelineKey,
) -> wgpu::RenderPipeline {
    let component = |b: Blend| wgpu::BlendComponent {
        src_factor: blend_factor(b.src),
        dst_factor: blend_factor(b.dst),
        operation: wgpu::BlendOperation::Add,
    };
    let entry = if key.blend.is_some() {
        "fs_blend"
    } else if key.alpha_test {
        "fs_test"
    } else {
        "fs_opaque"
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("md3 props"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ModelVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: key.blend.map(|b| wgpu::BlendState { color: component(b), alpha: component(b) }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(key.depth_write),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_props_do_not_move() {
        let p = Prop::for_classname("team_ctf_alphaflag", [1.0, 2.0, 3.0], 90.0).unwrap();
        assert!(!p.animated);
        assert_eq!(prop_matrix(&p, 0.0), prop_matrix(&p, 5.0));
        let item = Prop::for_classname("item_armor_ra", [0.0; 3], 0.0).unwrap();
        assert!(item.animated);
        assert_ne!(prop_matrix(&item, 0.0), prop_matrix(&item, 1.0));
    }

    #[test]
    fn extension_stripping() {
        assert_eq!(strip_extension("models/a/b.tga"), "models/a/b");
        assert_eq!(strip_extension("models/a.b/c"), "models/a.b/c");
    }
}
