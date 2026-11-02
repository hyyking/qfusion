//! Offscreen rendering of the world to a PNG (`--screenshot`), without a window.

use std::path::Path;

use qf_engine::content::bsp::{Bsp, WorldMesh};

use crate::model_render::{ModelRenderer, props_from_entities};
use crate::render::{begin_world_pass, create_depth_view, view_projection};
use crate::world::{Content, WorldRenderer};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub fn screenshot(
    path: &Path,
    content: &Content,
    bsp: &Bsp,
    mesh: &WorldMesh,
    eye: [f32; 3],
    view_angles: [f32; 3],
    (width, height): (u32, u32),
) -> Result<(), Box<dyn std::error::Error>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

    let mut world = WorldRenderer::new(&device, &queue, FORMAT, content, bsp, mesh, &[])?;
    let models = ModelRenderer::new(
        &device,
        &queue,
        FORMAT,
        content,
        props_from_entities(&bsp.parsed_entities),
    );
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("screenshot"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let depth = create_depth_view(&device, width, height);
    let mvp = view_projection(eye, view_angles, width as f32 / height as f32);
    world.prepare(&queue, mvp, eye, 0.0, &[]);
    models.prepare(&queue, mvp, eye, 0.0);

    let row_bytes = width * 4;
    let padded = row_bytes.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("screenshot readback"),
        size: u64::from(padded * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = begin_world_pass(&mut encoder, &view, &depth);
        world.draw(&mut pass);
        models.draw(&mut pass);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map screenshot buffer"));
    device.poll(wgpu::PollType::wait_indefinitely())?;
    let mapped = slice.get_mapped_range()?;
    let mut pixels = Vec::with_capacity((row_bytes * height) as usize);
    for row in mapped.chunks(padded as usize).take(height as usize) {
        pixels.extend_from_slice(&row[..row_bytes as usize]);
    }
    drop(mapped);
    image::save_buffer(path, &pixels, width, height, image::ColorType::Rgba8)?;
    Ok(())
}
