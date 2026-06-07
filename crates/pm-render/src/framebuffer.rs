//! Port of `Renderer/Framebuffer.{hpp,cpp}`, adapted to wgpu.
//!
//! projectM uses a `Framebuffer` holding several render targets that act as
//! both draw destinations and sampling sources — most importantly the
//! current/previous-frame ping-pong used by the warp shader. OpenGL needs an
//! explicit "bind FBO" step; in wgpu you instead target a texture's view in a
//! render pass, so this type just owns the target textures and hands out views.

use crate::texture::{Texture, TARGET_FORMAT};

/// A set of equally-sized color render targets.
pub struct Framebuffer {
    buffers: Vec<Texture>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
}

impl Framebuffer {
    /// Create `count` color targets (>= 1) of the given size.
    pub fn new(device: &wgpu::Device, count: usize, width: u32, height: u32) -> Self {
        Self::with_format(device, count, width, height, TARGET_FORMAT)
    }

    pub fn with_format(
        device: &wgpu::Device,
        count: usize,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let count = count.max(1);
        let buffers = (0..count)
            .map(|i| Texture::new_render_target(device, format!("framebuffer[{i}]"), width, height, format))
            .collect();
        Framebuffer { buffers, width: width.max(1), height: height.max(1), format }
    }

    pub fn count(&self) -> usize {
        self.buffers.len()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// The color target at `index`.
    pub fn texture(&self, index: usize) -> &Texture {
        &self.buffers[index]
    }

    /// The render/sample view of the color target at `index`.
    pub fn view(&self, index: usize) -> &wgpu::TextureView {
        &self.buffers[index].view
    }

    /// Reallocate all targets to a new size. Returns `false` (no change) if the
    /// size is degenerate or identical to the current size, matching upstream.
    pub fn set_size(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 || (width == self.width && height == self.height) {
            return false;
        }
        let count = self.buffers.len();
        self.buffers = (0..count)
            .map(|i| Texture::new_render_target(device, format!("framebuffer[{i}]"), width, height, self.format))
            .collect();
        self.width = width;
        self.height = height;
        true
    }
}
