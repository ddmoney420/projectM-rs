//! CPU readback of a GPU texture — used for offscreen screenshots and tests.

use crate::context::GpuContext;
use crate::texture::Texture;

/// Copy an RGBA8 texture back to CPU memory, returning tightly-packed
/// `width * height * 4` bytes (row padding removed).
///
/// The texture must be RGBA8 and have `COPY_SRC` usage (render targets created
/// via [`Texture::new_render_target`] do).
pub fn read_rgba8(ctx: &GpuContext, texture: &Texture) -> Vec<u8> {
    let width = texture.width;
    let height = texture.height;
    let unpadded_bytes_per_row = width * 4;

    // copy_texture_to_buffer requires bytes_per_row aligned to 256.
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

    let buffer_size = (padded_bytes_per_row * height) as wgpu::BufferAddress;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback buffer"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = ctx
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback encoder") });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture.texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    ctx.queue.submit(Some(encoder.finish()));

    // Map and wait.
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = ctx
        .device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    rx.recv().expect("map channel closed").expect("buffer map failed");

    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((unpadded_bytes_per_row * height) as usize);
    for row in 0..height {
        let start = (row * padded_bytes_per_row) as usize;
        let end = start + unpadded_bytes_per_row as usize;
        out.extend_from_slice(&data[start..end]);
    }
    drop(data);
    buffer.unmap();

    out
}
