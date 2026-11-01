//! wgpu renderer for the BSP reader's parsed world mesh.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;
use winit::window::Window;

use qf_engine::content::bsp::WorldMesh;
use qf_engine::simulation::entities::MoverPose;

use crate::hud::Canvas;
use crate::math;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    mvp: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HudVertex {
    position: [f32; 2],
    color: [f32; 4],
}

/// 2D overlay drawn after the world: coloured rectangles in pixel coordinates.
struct HudPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertex_buf: wgpu::Buffer,
    capacity: usize,
    vertices: Vec<HudVertex>,
}

impl HudPass {
    const INITIAL_CAPACITY: usize = 4096;

    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HUD"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hud.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HUD screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HUD"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HUD"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<HudVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HUD screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("HUD screen"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            uniform_buf,
            bind_group,
            vertex_buf: Self::vertex_buffer(device, Self::INITIAL_CAPACITY),
            capacity: Self::INITIAL_CAPACITY,
            vertices: Vec::with_capacity(Self::INITIAL_CAPACITY),
        }
    }

    fn vertex_buffer(device: &wgpu::Device, vertices: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("HUD vertices"),
            size: (vertices * std::mem::size_of::<HudVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads the canvas; returns the vertex count to draw.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        canvas: &Canvas,
        size: [f32; 2],
    ) -> u32 {
        self.vertices.clear();
        for r in &canvas.rects {
            let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
            for [x, y] in [[x0, y0], [x1, y0], [x1, y1], [x0, y0], [x1, y1], [x0, y1]] {
                self.vertices.push(HudVertex {
                    position: [x, y],
                    color: r.color,
                });
            }
        }
        if self.vertices.len() > self.capacity {
            self.capacity = self.vertices.len().next_power_of_two();
            self.vertex_buf = Self::vertex_buffer(device, self.capacity);
        }
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::cast_slice(&[size[0], size[1], 0.0, 0.0]),
        );
        queue.write_buffer(&self.vertex_buf, 0, bytemuck::cast_slice(&self.vertices));
        self.vertices.len() as u32
    }
}

pub struct Renderer {
    hud: HudPass,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    depth_view: wgpu::TextureView,
    submodels: Vec<(usize, WorldMesh)>,
    mover_vertex_buf: wgpu::Buffer,
    mover_index_buf: wgpu::Buffer,
    mover_vertices: Vec<Vertex>,
    mover_indices: Vec<u32>,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        mesh: &WorldMesh,
        submodels: Vec<(usize, WorldMesh)>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if mesh.positions.is_empty() || mesh.indices.is_empty() {
            return Err("parsed BSP world mesh contains no renderable triangles".into());
        }
        if mesh.positions.len() != mesh.normals.len() || !mesh.indices.len().is_multiple_of(3) {
            return Err("parsed BSP world mesh has inconsistent vertex or index data".into());
        }
        let index_count = u32::try_from(mesh.indices.len())?;
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface is not supported by the selected adapter")?;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BSP world"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bsp.wgsl").into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("BSP view uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("BSP world"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("BSP world"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(config.format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let vertices: Vec<_> = mesh
            .positions
            .iter()
            .zip(&mesh.normals)
            .map(|(&position, &normal)| Vertex { position, normal })
            .collect();
        let vertex_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP world vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("BSP world indices"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP view uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("BSP view uniforms"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let depth_view = create_depth_view(&device, &config);
        let hud = HudPass::new(&device, config.format);
        let vertex_capacity: usize = submodels.iter().map(|(_, m)| m.positions.len()).sum();
        let index_capacity: usize = submodels.iter().map(|(_, m)| m.indices.len()).sum();
        let mover_vertex_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP mover vertices"),
            size: (vertex_capacity.max(1) * std::mem::size_of::<Vertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mover_index_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("BSP mover indices"),
            size: (index_capacity.max(1) * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            hud,
            surface,
            device,
            queue,
            config,
            pipeline,
            vertex_buf,
            index_buf,
            index_count,
            uniform_buf,
            bind_group,
            depth_view,
            submodels,
            mover_vertex_buf,
            mover_index_buf,
            mover_vertices: Vec::new(),
            mover_indices: Vec::new(),
        })
    }

    /// Re-poses every mover's inline model on the CPU; the meshes are tiny next to the world.
    fn upload_movers(&mut self, movers: &[MoverPose]) {
        self.mover_vertices.clear();
        self.mover_indices.clear();
        for pose in movers {
            let Some((_, mesh)) = self.submodels.iter().find(|(m, _)| *m == pose.model) else {
                continue;
            };
            let base = self.mover_vertices.len() as u32;
            for (&position, &normal) in mesh.positions.iter().zip(&mesh.normals) {
                let rotated = pose.local_to_world([normal[0], normal[1], normal[2]]);
                let normal = std::array::from_fn(|i| rotated[i] - pose.origin[i]);
                self.mover_vertices.push(Vertex {
                    position: pose.local_to_world(position),
                    normal,
                });
            }
            self.mover_indices
                .extend(mesh.indices.iter().map(|&i| base + i));
        }
        if self.mover_indices.is_empty() {
            return;
        }
        self.queue.write_buffer(
            &self.mover_vertex_buf,
            0,
            bytemuck::cast_slice(&self.mover_vertices),
        );
        self.queue.write_buffer(
            &self.mover_index_buf,
            0,
            bytemuck::cast_slice(&self.mover_indices),
        );
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, &self.config);
    }

    pub fn size(&self) -> [f32; 2] {
        [self.config.width as f32, self.config.height as f32]
    }

    pub fn render(
        &mut self,
        eye: [f32; 3],
        view_angles: [f32; 3],
        movers: &[MoverPose],
        hud: &Canvas,
    ) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            // Timeout/occluded/etc.: skip this frame.
            _ => return,
        };

        let aspect = self.config.width as f32 / self.config.height as f32;
        let fov_y = std::f32::consts::FRAC_PI_2;
        let [pitch, yaw, _] = view_angles.map(f32::to_radians);
        let forward = [
            pitch.cos() * yaw.cos(),
            pitch.cos() * yaw.sin(),
            pitch.sin(),
        ];
        let target = [
            eye[0] + forward[0],
            eye[1] + forward[1],
            eye[2] + forward[2],
        ];
        let view = math::look_at(eye, target, [0.0, 0.0, 1.0]);
        let proj = math::perspective(fov_y, aspect, 0.1, 16_384.0);
        let mvp = math::mul(&proj, &view);
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&Uniforms { mvp }));
        self.upload_movers(movers);

        let color_view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("BSP world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.06,
                            b: 0.09,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, self.vertex_buf.slice(..));
            pass.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
            if !self.mover_indices.is_empty() {
                pass.set_vertex_buffer(0, self.mover_vertex_buf.slice(..));
                pass.set_index_buffer(self.mover_index_buf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.mover_indices.len() as u32, 0, 0..1);
            }
        }
        let size = self.size();
        let hud_vertices = self.hud.prepare(&self.device, &self.queue, hud, size);
        if hud_vertices > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HUD"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.hud.pipeline);
            pass.set_bind_group(0, &self.hud.bind_group, &[]);
            pass.set_vertex_buffer(0, self.hud.vertex_buf.slice(..));
            pass.draw(0..hud_vertices, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

fn create_depth_view(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
