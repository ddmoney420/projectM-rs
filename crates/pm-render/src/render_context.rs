//! Port of `Renderer/RenderContext.hpp` — per-frame global values threaded
//! through preset rendering.

/// Holds the global, per-frame rendering values a preset reads. Pure data,
/// updated each frame by the host/orchestrator before rendering.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Seconds since the current preset started.
    pub time: f32,
    /// Frames rendered so far.
    pub frame: i32,
    /// Frames per second.
    pub fps: f32,
    /// Preset progress, 0..1.
    pub progress: f32,
    /// Preset transition / blend progress, 0..1.
    pub blend_progress: f32,

    pub viewport_size_x: i32,
    pub viewport_size_y: i32,

    pub aspect_x: f32,
    pub aspect_y: f32,
    pub inv_aspect_x: f32,
    pub inv_aspect_y: f32,

    /// Per-pixel mesh resolution.
    pub per_pixel_mesh_x: i32,
    pub per_pixel_mesh_y: i32,

    /// Texel offsets used by the warp shader.
    pub texel_offset_x: f32,
    pub texel_offset_y: f32,
}

impl Default for RenderContext {
    fn default() -> Self {
        RenderContext {
            time: 0.0,
            frame: 0,
            fps: 0.0,
            progress: 0.0,
            blend_progress: 0.0,
            viewport_size_x: 0,
            viewport_size_y: 0,
            aspect_x: 1.0,
            aspect_y: 1.0,
            inv_aspect_x: 1.0,
            inv_aspect_y: 1.0,
            per_pixel_mesh_x: 64,
            per_pixel_mesh_y: 48,
            texel_offset_x: 0.0,
            texel_offset_y: 0.0,
        }
    }
}

impl RenderContext {
    /// Set viewport size and derive aspect ratios the way the C++ renderer does:
    /// the larger dimension is normalized to 1.0.
    pub fn set_viewport(&mut self, width: i32, height: i32) {
        self.viewport_size_x = width;
        self.viewport_size_y = height;

        if width > height {
            self.aspect_x = width as f32 / height.max(1) as f32;
            self.aspect_y = 1.0;
        } else {
            self.aspect_x = 1.0;
            self.aspect_y = height as f32 / width.max(1) as f32;
        }
        self.inv_aspect_x = 1.0 / self.aspect_x;
        self.inv_aspect_y = 1.0 / self.aspect_y;
    }
}
