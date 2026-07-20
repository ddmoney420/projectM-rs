//! Port of `Renderer/Texture.{hpp,cpp}` — a GPU texture that can be both a
//! render target and a sampling source, mirroring how projectM framebuffers
//! double as textures.

/// Default internal format for render targets. `Unorm` (not `Srgb`) so that
/// clear/draw values map directly to stored bytes — important for predictable
/// readback and for matching Milkdrop's linear blending math.
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A GPU texture plus its default view and metadata.
pub struct Texture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
    /// Depth (array layers) — `1` for an ordinary 2D texture, `>1` for a 3D
    /// volume texture (e.g. Milkdrop's `noisevol_*`).
    pub depth: u32,
    /// Name used to reference the texture from Milkdrop shaders.
    pub name: String,
}

impl Texture {
    /// Allocate an empty 2D texture usable as a render target and as a sampled
    /// texture, and copyable for readback.
    pub fn new_render_target(
        device: &wgpu::Device,
        name: impl Into<String>,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let name = name.into();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&name),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        register(Texture { texture, view, format, width: width.max(1), height: height.max(1), depth: 1, name })
    }

    /// Create a sampled texture from tightly-packed RGBA8 pixel data
    /// (`width * height * 4` bytes), e.g. a user image loaded by a preset.
    pub fn from_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: impl Into<String>,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Self {
        assert_eq!(
            data.len(),
            (width * height * 4) as usize,
            "rgba8 data length must equal width*height*4"
        );
        let name = name.into();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&name),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        register(Texture { texture, view, format, width, height, depth: 1, name })
    }

    /// Create a sampled 3D (volume) texture from tightly-packed RGBA8 data
    /// (`width * height * depth * 4` bytes), e.g. Milkdrop's `noisevol_*`.
    pub fn from_rgba8_3d(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: impl Into<String>,
        width: u32,
        height: u32,
        depth: u32,
        data: &[u8],
    ) -> Self {
        assert_eq!(
            data.len(),
            (width * height * depth * 4) as usize,
            "rgba8 3d data length must equal width*height*depth*4"
        );
        let name = name.into();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: depth };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&name),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        register(Texture { texture, view, format, width, height, depth, name })
    }

    /// True if this is a 3D (volume) texture.
    pub fn is_3d(&self) -> bool {
        self.depth > 1
    }

    /// True if the texture has no allocated size.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Estimated base-level byte size (no mips): w × h × depth × bpp.
    pub fn estimated_bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * u64::from(self.depth) * u64::from(bytes_per_pixel(self.format))
    }
}

// --- Phase 10D: live render-target accounting -------------------------------
//
// A MEASURED (not hand-estimated) count of live `pm-render::Texture`s and their
// total base-level byte size. Every constructor registers; `Drop` unregisters —
// so a leak (retained WarpEngine / Compositor / deck) shows up as a count that
// never returns to baseline. Exposed to the diagnostics/telemetry layer.

use std::sync::atomic::{AtomicI64, Ordering};

static LIVE_TEXTURES: AtomicI64 = AtomicI64::new(0);
static LIVE_TEXTURE_BYTES: AtomicI64 = AtomicI64::new(0);

pub(crate) fn bytes_per_pixel(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::R8Unorm => 1,
        wgpu::TextureFormat::Rgba16Float => 8,
        wgpu::TextureFormat::Rgba32Float => 16,
        // Rgba8/Bgra8 (unorm/srgb), R32Float, and other 32-bit targets.
        _ => 4,
    }
}

/// Number of live `Texture`s (render targets + sampled sources) right now.
pub fn live_texture_count() -> i64 {
    LIVE_TEXTURES.load(Ordering::Relaxed)
}

/// Total estimated base-level bytes across all live `Texture`s.
pub fn live_texture_bytes() -> i64 {
    LIVE_TEXTURE_BYTES.load(Ordering::Relaxed)
}

/// Register a freshly-built texture in the live accounting. All constructors
/// route their return value through this.
fn register(t: Texture) -> Texture {
    LIVE_TEXTURES.fetch_add(1, Ordering::Relaxed);
    LIVE_TEXTURE_BYTES.fetch_add(t.estimated_bytes() as i64, Ordering::Relaxed);
    t
}

impl Drop for Texture {
    fn drop(&mut self) {
        LIVE_TEXTURES.fetch_sub(1, Ordering::Relaxed);
        LIVE_TEXTURE_BYTES.fetch_sub(self.estimated_bytes() as i64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpp_matches_known_formats() {
        assert_eq!(bytes_per_pixel(wgpu::TextureFormat::Rgba8Unorm), 4);
        assert_eq!(bytes_per_pixel(wgpu::TextureFormat::Bgra8UnormSrgb), 4);
        assert_eq!(bytes_per_pixel(wgpu::TextureFormat::R8Unorm), 1);
        assert_eq!(bytes_per_pixel(wgpu::TextureFormat::Rgba16Float), 8);
        assert_eq!(bytes_per_pixel(wgpu::TextureFormat::Rgba32Float), 16);
    }

    #[test]
    fn estimated_bytes_is_w_h_depth_bpp() {
        // 1920×1080 Rgba8 = ~8.29 MB; a 3-layer volume triples it.
        assert_eq!(1920u64 * 1080 * 1 * 4, 8_294_400);
        assert_eq!(256u64 * 256 * 4 * 4, 1_048_576); // 4-layer noisevol
    }
}
