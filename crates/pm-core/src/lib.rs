//! `pm-core` — orchestrates a Milkdrop preset into rendered frames.
//!
//! Brings the pieces together: [`pm_preset`] evaluates the equations,
//! [`crate::warp_mesh`] builds the per-vertex warp geometry, and
//! [`crate::warp_render`] runs the GPU warp pass on [`pm_render`]'s wgpu device.
//!
//! Currently implements the **warp/motion feedback** stage (the core of
//! Milkdrop's "flow"). Composite, waveform, shape and border passes build on
//! this in later work.

mod warp_mesh;
mod warp_render;

pub use warp_mesh::{WarpMesh, WarpVertex};
pub use warp_render::{WarpParams, WarpRenderer};

use pm_audio::FrameAudioData;
use pm_preset::{FrameParams, Preset, PresetError, PresetState};
use pm_render::{GpuContext, Texture};

const DEFAULT_MESH_X: usize = 64;
const DEFAULT_MESH_Y: usize = 48;

/// Aspect ratios for a viewport, matching projectM's convention (the larger
/// dimension normalizes to 1.0). Returns `(aspectX, aspectY, invAspectX, invAspectY)`.
fn compute_aspect(width: u32, height: u32) -> (f32, f32, f32, f32) {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    if width > height {
        (w / h, 1.0, h / w, 1.0)
    } else {
        (1.0, h / w, 1.0, w / h)
    }
}

/// Warp parameters derived from the current preset state, mirroring
/// `PerPixelMesh::WarpedBlit`.
fn warp_params(state: &PresetState) -> WarpParams {
    let warp_time = state.frame.time * state.warp_anim_speed;
    let warp_scale_inverse = 1.0 / state.warp_scale;
    let warp_factors = [
        11.68 + 4.0 * (warp_time * 1.413 + 10.0).cos(),
        8.77 + 3.0 * (warp_time * 1.113 + 7.0).cos(),
        10.54 + 3.0 * (warp_time * 1.233 + 3.0).cos(),
        11.49 + 4.0 * (warp_time * 0.933 + 5.0).cos(),
    ];
    WarpParams {
        aspect: [
            state.frame.aspect_x,
            state.frame.aspect_y,
            state.frame.inv_aspect_x,
            state.frame.inv_aspect_y,
        ],
        warp_factors,
        texel_offset: [0.0, 0.0],
        warp_time,
        warp_scale_inverse,
        decay: state.decay.min(1.0),
        wrap: state.tex_wrap,
    }
}

/// Drives one preset's warp/motion render at a fixed resolution.
pub struct WarpEngine {
    preset: Preset,
    mesh: WarpMesh,
    renderer: WarpRenderer,
    width: u32,
    height: u32,
    aspect: (f32, f32, f32, f32),
    mesh_x: usize,
    mesh_y: usize,
}

impl WarpEngine {
    pub fn new(ctx: &GpuContext, preset: Preset, width: u32, height: u32) -> Self {
        let aspect = compute_aspect(width, height);
        let mesh = WarpMesh::new(DEFAULT_MESH_X, DEFAULT_MESH_Y, aspect.0, aspect.1);
        let renderer = WarpRenderer::new(ctx, width, height);
        WarpEngine {
            preset,
            mesh,
            renderer,
            width,
            height,
            aspect,
            mesh_x: DEFAULT_MESH_X,
            mesh_y: DEFAULT_MESH_Y,
        }
    }

    /// Seed the feedback buffer with an initial RGBA8 image
    /// (`width * height * 4` bytes).
    pub fn seed(&self, ctx: &GpuContext, rgba: &[u8]) {
        self.renderer.seed(ctx, rgba);
    }

    /// Render one frame: evaluate the preset for `time`/`audio`, compute the
    /// warp mesh, and run the warp pass. The result is in [`Self::main_texture`].
    pub fn render_frame(
        &mut self,
        ctx: &GpuContext,
        time: f32,
        frame: i32,
        audio: FrameAudioData,
    ) -> Result<(), PresetError> {
        let frame_params = FrameParams {
            time,
            fps: 60.0,
            frame,
            progress: 0.0,
            viewport_width: self.width as i32,
            viewport_height: self.height as i32,
            aspect_x: self.aspect.0,
            aspect_y: self.aspect.1,
            inv_aspect_x: self.aspect.2,
            inv_aspect_y: self.aspect.3,
            mesh_x: self.mesh_x as i32,
            mesh_y: self.mesh_y as i32,
        };

        self.preset.update_frame(frame_params, audio)?;
        self.mesh.calculate(&mut self.preset)?;

        let params = warp_params(self.preset.state());
        self.renderer.warp_frame(ctx, &self.mesh, &params);
        Ok(())
    }

    /// The texture holding the most recently rendered frame.
    pub fn main_texture(&self) -> &Texture {
        self.renderer.main_texture()
    }

    pub fn state(&self) -> &PresetState {
        self.preset.state()
    }
}
