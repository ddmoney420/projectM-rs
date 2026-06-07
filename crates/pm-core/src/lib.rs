//! `pm-core` — orchestrates a Milkdrop preset into rendered frames.
//!
//! Brings the pieces together: [`pm_preset`] evaluates the equations,
//! [`crate::warp_mesh`] builds the per-vertex warp geometry,
//! [`crate::warp_render`] runs the GPU warp/feedback pass, [`crate::waveform`]
//! generates the audio waveform that's drawn into the feedback buffer, and
//! [`crate::composite`] does the final tinted draw to the display target.
//!
//! Per-frame pipeline (mirrors Milkdrop):
//! `warp (feedback) → waveform → composite`.

mod composite;
mod warp_mesh;
mod warp_render;
mod waveform;
mod waveform_render;

pub use composite::CompositeRenderer;
pub use warp_mesh::{WarpMesh, WarpVertex};
pub use warp_render::{WarpParams, WarpRenderer};
pub use waveform::{generate as generate_waveform, WaveformGeometry};
pub use waveform_render::WaveformRenderer;

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

/// Warp parameters derived from the current preset state (`PerPixelMesh::WarpedBlit`).
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
        aspect: [state.frame.aspect_x, state.frame.aspect_y, state.frame.inv_aspect_x, state.frame.inv_aspect_y],
        warp_factors,
        texel_offset: [0.0, 0.0],
        warp_time,
        warp_scale_inverse,
        decay: state.decay.min(1.0),
        wrap: state.tex_wrap,
    }
}

/// Compute the waveform draw color (port of `Waveform::MaximizeColors`,
/// without the spiro-mode alpha scaling).
fn waveform_color(state: &PresetState) -> [f32; 4] {
    let mut alpha = state.wave_alpha;

    if state.mod_wave_alpha_by_volume {
        let vol = state.audio.vol;
        alpha = if vol <= state.mod_wave_alpha_start {
            0.0
        } else if vol >= state.mod_wave_alpha_end {
            state.wave_alpha
        } else {
            state.wave_alpha * (vol - state.mod_wave_alpha_start)
                / (state.mod_wave_alpha_end - state.mod_wave_alpha_start)
        };
    }
    alpha = alpha.clamp(0.0, 1.0);

    let (mut r, mut g, mut b) = (state.wave_r, state.wave_g, state.wave_b);
    if state.maximize_wave_color {
        let max = r.max(g).max(b);
        if max > 0.01 {
            r /= max;
            g /= max;
            b /= max;
        }
    }
    [r, g, b, alpha]
}

/// Drives one preset's full render (warp + waveform + composite) at a fixed
/// resolution.
pub struct WarpEngine {
    preset: Preset,
    mesh: WarpMesh,
    warp: WarpRenderer,
    waveform: WaveformRenderer,
    composite: CompositeRenderer,
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
        WarpEngine {
            preset,
            mesh,
            warp: WarpRenderer::new(ctx, width, height),
            waveform: WaveformRenderer::new(ctx),
            composite: CompositeRenderer::new(ctx, width, height),
            width,
            height,
            aspect,
            mesh_x: DEFAULT_MESH_X,
            mesh_y: DEFAULT_MESH_Y,
        }
    }

    /// Seed the feedback buffer with an initial RGBA8 image.
    pub fn seed(&self, ctx: &GpuContext, rgba: &[u8]) {
        self.warp.seed(ctx, rgba);
    }

    /// Render one full frame: evaluate the preset, warp the feedback buffer,
    /// draw the waveform into it, and composite to the display target.
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

        // 1. Equations + warp/feedback.
        self.preset.update_frame(frame_params, audio)?;
        self.mesh.calculate(&mut self.preset)?;
        let params = warp_params(self.preset.state());
        self.warp.warp_frame(ctx, &self.mesh, &params);

        // 2. Waveform drawn into the (now warped) feedback buffer.
        let geometry = generate_waveform(self.preset.state());
        let color = waveform_color(self.preset.state());
        self.waveform.draw(
            ctx,
            self.warp.current_view(),
            &geometry.points,
            color,
            self.preset.state().additive_waves,
            geometry.is_loop,
        );

        // 3. Composite to the display target.
        let shades = composite::hue_shades(time, self.preset.state().hue_random_offsets);
        self.composite.draw(ctx, self.warp.main_texture(), shades);
        Ok(())
    }

    /// The feedback buffer (input to the next frame's warp).
    pub fn main_texture(&self) -> &Texture {
        self.warp.main_texture()
    }

    /// The final composited image for display / screenshot.
    pub fn display_texture(&self) -> &Texture {
        self.composite.output()
    }

    pub fn state(&self) -> &PresetState {
        self.preset.state()
    }
}
