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

        Texture { texture, view, format, width: width.max(1), height: height.max(1), name }
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
        Texture { texture, view, format, width, height, name }
    }

    /// True if the texture has no allocated size.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}
