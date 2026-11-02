//! wgpu renderer for the BSP reader's parsed world mesh.

use std::sync::Arc;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use qf_engine::content::bsp::{Bsp, WorldMesh};
use qf_engine::simulation::entities::MoverPose;

use crate::hud::Canvas;
use crate::math;
use crate::model_render::{ModelRenderer, props_from_entities};
use crate::world::{Content, DEPTH_FORMAT, WorldRenderer};

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
    world: WorldRenderer,
    models: ModelRenderer,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// Non-sRGB view format: the legacy renderer blends in gamma space.
    world_format: wgpu::TextureFormat,
    depth_view: wgpu::TextureView,
    start: Instant,
}

impl Renderer {
    pub fn new(
        window: Arc<Window>,
        content: &Content,
        bsp: &Bsp,
        mesh: &WorldMesh,
        submodels: Vec<(usize, WorldMesh)>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if mesh.positions.is_empty() || mesh.indices.is_empty() {
            return Err("parsed BSP world mesh contains no renderable triangles".into());
        }
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
        let world_format = config.format.remove_srgb_suffix();
        if world_format != config.format {
            config.view_formats.push(world_format);
        }
        surface.configure(&device, &config);

        let world = WorldRenderer::new(
            &device,
            &queue,
            world_format,
            content,
            bsp,
            mesh,
            &submodels,
        )?;
        let models = ModelRenderer::new(
            &device,
            &queue,
            world_format,
            content,
            props_from_entities(&bsp.parsed_entities),
        );
        let depth_view = create_depth_view(&device, config.width, config.height);
        let hud = HudPass::new(&device, config.format);
        Ok(Self {
            hud,
            world,
            models,
            surface,
            device,
            queue,
            config,
            world_format,
            depth_view,
            start: Instant::now(),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = create_depth_view(&self.device, width, height);
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
        let mvp = view_projection(eye, view_angles, aspect);
        let time = self.start.elapsed().as_secs_f32();
        self.world.prepare(&self.queue, mvp, eye, time, movers);
        self.models.prepare(&self.queue, mvp, eye, time);

        let world_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.world_format),
            ..Default::default()
        });
        let color_view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_world_pass(&mut encoder, &world_view, &self.depth_view);
            self.world.draw(&mut pass);
            self.models.draw(&mut pass);
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

/// Perspective view-projection for the persistent client view angles (pitch, yaw, roll).
pub fn view_projection(eye: [f32; 3], view_angles: [f32; 3], aspect: f32) -> [[f32; 4]; 4] {
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
    let proj = math::perspective(fov_y, aspect, 1.0, 16_384.0);
    math::mul(&proj, &view)
}

pub fn begin_world_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("BSP world"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
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
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0.0),
                store: wgpu::StoreOp::Discard,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

pub fn create_depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
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
