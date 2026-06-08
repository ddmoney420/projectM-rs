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

mod blur;
mod border;
mod colored_line;
mod composite;
mod md_uniforms;
mod motion_vectors;
mod noise;
mod preset_composite;
mod preset_warp;
mod warp_mesh;
mod warp_render;
mod waveform;
mod waveform_render;

pub use border::{frames as border_frames, BorderFrame};
pub use colored_line::ColoredLineRenderer;
pub use composite::CompositeRenderer;

/// The classic (non-shader) final-composite effect parameters, read from the
/// preset state each frame: video echo, gamma brighten, and the colour filters.
#[derive(Debug, Clone, Copy)]
pub struct CompositeEffects {
    pub echo_zoom: f32,
    pub echo_alpha: f32,
    pub echo_orientation: i32,
    pub gamma: f32,
    pub brighten: bool,
    pub darken: bool,
    pub solarize: bool,
    pub invert: bool,
}

impl CompositeEffects {
    fn from_state(state: &PresetState) -> Self {
        CompositeEffects {
            echo_zoom: state.video_echo_zoom,
            echo_alpha: state.video_echo_alpha,
            echo_orientation: state.video_echo_orientation,
            gamma: state.gamma_adj,
            brighten: state.brighten,
            darken: state.darken,
            solarize: state.solarize,
            invert: state.invert,
        }
    }
}
pub use preset_composite::PresetComposite;
pub use warp_mesh::{WarpMesh, WarpVertex};
pub use warp_render::{WarpParams, WarpRenderer};
pub use waveform::{generate as generate_waveform, WaveformGeometry};
pub use waveform_render::WaveformRenderer;

use pm_audio::FrameAudioData;
use pm_preset::{shader_to_wgsl, FrameParams, Preset, PresetError, PresetState, ShaderKind};
use pm_render::{GpuContext, Texture};

/// Try to build a custom composite renderer from the preset's composite shader.
fn build_preset_composite(
    ctx: &GpuContext,
    preset: &Preset,
    width: u32,
    height: u32,
) -> Option<PresetComposite> {
    let src = preset.composite_shader_source();
    if !src.contains("shader_body") {
        return None;
    }
    let translated = shader_to_wgsl(src, ShaderKind::Composite).ok()?;
    PresetComposite::new(ctx, &translated, width, height)
}

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
    custom_lines: ColoredLineRenderer,
    composite: CompositeRenderer,
    /// The preset's custom composite shader, if it translated successfully.
    preset_composite: Option<PresetComposite>,
    /// Milkdrop's built-in noise textures, bound to `sampler_noise*` references.
    noise: noise::NoiseTextures,
    /// Per-frame Gaussian blur chain, bound to `sampler_blur1/2/3` references.
    blur: blur::Blur,
    /// Motion-vector grid overlay (`mv_*`).
    motion_vectors: motion_vectors::MotionVectors,
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
        let preset_composite = build_preset_composite(ctx, &preset, width, height);

        // Install the preset's custom warp shader, if it translates.
        let mut warp = WarpRenderer::new(ctx, width, height);
        let warp_src = preset.warp_shader_source();
        if warp_src.contains("shader_body") {
            if let Ok(parts) = pm_preset::warp_shader_parts(warp_src) {
                warp.set_custom_warp(ctx, &parts);
            }
        }

        WarpEngine {
            preset,
            mesh,
            warp,
            waveform: WaveformRenderer::new(ctx),
            custom_lines: ColoredLineRenderer::new(ctx),
            composite: CompositeRenderer::new(ctx, width, height),
            preset_composite,
            noise: noise::NoiseTextures::new(ctx),
            blur: blur::Blur::new(ctx, width, height),
            motion_vectors: motion_vectors::MotionVectors::new(ctx),
            width,
            height,
            aspect,
            mesh_x: DEFAULT_MESH_X,
            mesh_y: DEFAULT_MESH_Y,
        }
    }

    /// Whether this preset is rendering its own (custom) composite shader.
    pub fn uses_custom_composite(&self) -> bool {
        self.preset_composite.is_some()
    }

    /// Whether this preset is rendering its own (custom) warp shader.
    pub fn uses_custom_warp(&self) -> bool {
        self.warp.has_custom_warp()
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
        // Blur chain from the current feedback (last frame's image), so warp and
        // composite can sample GetBlur1/2/3 this frame.
        self.blur.compute(ctx, self.warp.main_texture());
        // The custom warp shader needs the per-frame MdUniforms block.
        let md = if self.warp.has_custom_warp() {
            Some(md_uniforms::MdUniforms::from_state(self.preset.state(), time))
        } else {
            None
        };
        self.warp.warp_frame(ctx, &self.mesh, &params, md.as_ref().map(bytemuck::bytes_of), &self.noise, &self.blur);

        // 1a. Motion vectors visualising the warp flow, into the feedback buffer.
        self.motion_vectors.draw(ctx, self.warp.current_view(), self.warp.motion_texture(), self.preset.state());

        // 1b. Custom shapes (filled N-gons + borders) into the feedback buffer.
        let shapes = self.preset.custom_shapes()?;
        for sh in &shapes {
            self.custom_lines.draw_triangles(
                ctx,
                self.warp.current_view(),
                &sh.fill_vertices,
                &sh.fill_colors,
                sh.additive,
            );
            if !sh.border_points.is_empty() {
                let border_colors = vec![sh.border_color; sh.border_points.len()];
                self.custom_lines.draw(
                    ctx,
                    self.warp.current_view(),
                    &sh.border_points,
                    &border_colors,
                    false,
                    false,
                );
            }
        }

        // 2. Standard waveform drawn into the (now warped) feedback buffer.
        let geometry = generate_waveform(self.preset.state());
        let color = waveform_color(self.preset.state());
        let additive = self.preset.state().additive_waves;
        self.waveform.draw(ctx, self.warp.current_view(), &geometry.points, color, additive, geometry.is_loop);
        // The double-line mode (7) draws a second polyline for the right channel.
        if let Some(points2) = &geometry.points2 {
            self.waveform.draw(ctx, self.warp.current_view(), points2, color, additive, geometry.is_loop);
        }

        // 2b. Custom waveforms (wave_N per-point geometry) on top.
        let customs = self.preset.custom_waveforms()?;
        for cw in &customs {
            self.custom_lines.draw(
                ctx,
                self.warp.current_view(),
                &cw.points,
                &cw.colors,
                cw.additive,
                cw.use_dots,
            );
        }

        // 2c. Inner/outer border frames, drawn last into the feedback buffer.
        for frame in border::frames(self.preset.state()) {
            self.custom_lines.draw_triangles(ctx, self.warp.current_view(), &frame.vertices, &frame.colors, false);
        }

        // 3. Composite to the display target: the preset's own composite shader
        //    if it has one, otherwise the built-in hue composite.
        if let Some(pc) = &self.preset_composite {
            pc.draw(ctx, self.warp.main_texture(), self.preset.state(), time, &self.noise, &self.blur);
        } else {
            let shades = composite::hue_shades(time, self.preset.state().hue_random_offsets);
            let fx = CompositeEffects::from_state(self.preset.state());
            self.composite.draw(ctx, self.warp.main_texture(), shades, fx);
        }
        Ok(())
    }

    /// The feedback buffer (input to the next frame's warp).
    pub fn main_texture(&self) -> &Texture {
        self.warp.main_texture()
    }

    /// The final composited image for display / screenshot.
    pub fn display_texture(&self) -> &Texture {
        match &self.preset_composite {
            Some(pc) => pc.output(),
            None => self.composite.output(),
        }
    }

    pub fn state(&self) -> &PresetState {
        self.preset.state()
    }
}
