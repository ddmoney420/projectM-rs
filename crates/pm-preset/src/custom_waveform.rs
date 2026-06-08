//! Port of `MilkdropPreset/CustomWaveform.{hpp,cpp}` and its per-frame /
//! per-point eval contexts.
//!
//! Each enabled `wave_N` waveform runs per-frame code (sets up q/t vars,
//! color), then per-point code over the (smoothed) audio samples to produce
//! `x`/`y`/`r`/`g`/`b`/`a` per vertex — arbitrary geometry the preset draws.

use crate::error::PresetError;
use crate::state::{PresetState, Q_VAR_COUNT, T_VAR_COUNT};
use pm_eval::{Context, Program};

const WAVEFORM_SAMPLES: i32 = pm_audio::WAVEFORM_SAMPLES as i32;
const SPECTRUM_SAMPLES: i32 = pm_audio::SPECTRUM_SAMPLES as i32;

/// Generated geometry for one custom waveform.
pub struct CustomWaveformOutput {
    pub points: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub additive: bool,
    pub use_dots: bool,
}

pub struct CustomWaveform {
    index: usize,
    per_frame: Context,
    per_point: Context,
    init_prog: Option<Program>,
    frame_prog: Option<Program>,
    point_prog: Option<Program>,
    t_after_init: [f64; T_VAR_COUNT],
}

impl CustomWaveform {
    /// Build the waveform if it's enabled. Returns `Ok(None)` for disabled ones.
    pub fn new(state: &PresetState, index: usize) -> Result<Option<Self>, PresetError> {
        if !state.custom_waveforms[index].enabled {
            return Ok(None);
        }
        let init_prog = compile_opt(&state.custom_wave_init_code[index], "wave_init")?;
        let frame_prog = compile_opt(&state.custom_wave_per_frame_code[index], "wave_per_frame")?;
        let point_prog = compile_opt(&state.custom_wave_per_point_code[index], "wave_per_point")?;

        let mut wf = CustomWaveform {
            index,
            per_frame: Context::new(),
            per_point: Context::new(),
            init_prog,
            frame_prog,
            point_prog,
            t_after_init: [0.0; T_VAR_COUNT],
        };
        wf.evaluate_init(state)?;
        Ok(Some(wf))
    }

    fn evaluate_init(&mut self, state: &PresetState) -> Result<(), PresetError> {
        self.load_per_frame_inputs(state, &[0.0; T_VAR_COUNT]);
        if let Some(p) = &self.init_prog {
            p.run(&mut self.per_frame).map_err(|e| PresetError::eval("wave_init", e))?;
        }
        for t in 0..T_VAR_COUNT {
            self.t_after_init[t] = self.per_frame.get(&format!("t{}", t + 1));
        }
        Ok(())
    }

    /// Load the per-frame inputs: audio/time read-only, the preset's q vars, and
    /// the waveform's color/sample properties.
    fn load_per_frame_inputs(&mut self, state: &PresetState, t_vars: &[f64; T_VAR_COUNT]) {
        let c = &mut self.per_frame;
        let cfg = &state.custom_waveforms[self.index];
        c.set("time", state.frame.time as f64);
        c.set("fps", state.frame.fps as f64);
        c.set("frame", state.frame.frame as f64);
        c.set("progress", state.frame.progress as f64);
        c.set("bass", state.audio.bass as f64);
        c.set("mid", state.audio.mid as f64);
        c.set("treb", state.audio.treb as f64);
        c.set("bass_att", state.audio.bass_att as f64);
        c.set("mid_att", state.audio.mid_att as f64);
        c.set("treb_att", state.audio.treb_att as f64);
        for (q, &value) in state.frame_q_variables.iter().enumerate() {
            c.set(&format!("q{}", q + 1), value);
        }
        for (t, &value) in t_vars.iter().enumerate() {
            c.set(&format!("t{}", t + 1), value);
        }
        c.set("r", cfg.r as f64);
        c.set("g", cfg.g as f64);
        c.set("b", cfg.b as f64);
        c.set("a", cfg.a as f64);
        c.set("samples", cfg.samples as f64);
    }

    /// Run this frame's per-frame + per-point code and produce the geometry.
    pub fn generate(&mut self, state: &PresetState) -> Result<Option<CustomWaveformOutput>, PresetError> {
        let cfg = state.custom_waveforms[self.index].clone();

        // Per-frame.
        let t_init = self.t_after_init;
        self.load_per_frame_inputs(state, &t_init);
        if let Some(p) = &self.frame_prog {
            p.run(&mut self.per_frame).map_err(|e| PresetError::eval("wave_per_frame", e))?;
        }

        let frame_rgba = [
            self.per_frame.get("r"),
            self.per_frame.get("g"),
            self.per_frame.get("b"),
            self.per_frame.get("a"),
        ];
        let samples_var = self.per_frame.get("samples");

        let max_count = if cfg.spectrum { SPECTRUM_SAMPLES } else { WAVEFORM_SAMPLES };
        let count = (samples_var as i32).clamp(0, max_count);
        if (cfg.use_dots && count < 1) || count < 2 {
            return Ok(None);
        }

        // Copy q/t into the per-point context (once) plus read-only inputs.
        for q in 0..Q_VAR_COUNT {
            let v = self.per_frame.get(&format!("q{}", q + 1));
            self.per_point.set(&format!("q{}", q + 1), v);
        }
        for t in 0..T_VAR_COUNT {
            let v = self.per_frame.get(&format!("t{}", t + 1));
            self.per_point.set(&format!("t{}", t + 1), v);
        }
        for (name, value) in [
            ("time", state.frame.time as f64),
            ("fps", state.frame.fps as f64),
            ("frame", state.frame.frame as f64),
            ("progress", state.frame.progress as f64),
            ("bass", state.audio.bass as f64),
            ("mid", state.audio.mid as f64),
            ("treb", state.audio.treb as f64),
            ("bass_att", state.audio.bass_att as f64),
            ("mid_att", state.audio.mid_att as f64),
            ("treb_att", state.audio.treb_att as f64),
        ] {
            self.per_point.set(name, value);
        }

        let (data_l, data_r) = self.smoothed_samples(state, &cfg, count, max_count);

        let inv_aspect_x = state.frame.inv_aspect_x;
        let inv_aspect_y = state.frame.inv_aspect_y;
        let mut points = Vec::with_capacity(count as usize);
        let mut colors = Vec::with_capacity(count as usize);
        let denom = if count > 1 { 1.0 / (count - 1) as f64 } else { 0.0 };

        for s in 0..count as usize {
            let sample_index = s as f64 * denom;
            let v1 = data_l[s] as f64;
            let v2 = data_r[s] as f64;
            let c = &mut self.per_point;
            c.set("sample", sample_index);
            c.set("value1", v1);
            c.set("value2", v2);
            c.set("x", 0.5 + v1);
            c.set("y", 0.5 + v2);
            c.set("r", frame_rgba[0]);
            c.set("g", frame_rgba[1]);
            c.set("b", frame_rgba[2]);
            c.set("a", frame_rgba[3]);

            if let Some(p) = &self.point_prog {
                p.run(c).map_err(|e| PresetError::eval("wave_per_point", e))?;
            }

            let x = c.get("x");
            let y = c.get("y");
            points.push([
                ((x * 2.0 - 1.0) * inv_aspect_x as f64) as f32,
                ((y * 2.0 - 1.0) * inv_aspect_y as f64) as f32,
            ]);
            colors.push([
                modulo(c.get("r")),
                modulo(c.get("g")),
                modulo(c.get("b")),
                modulo(c.get("a")),
            ]);
        }

        let (points, colors) = smooth_wave(&points, &colors);
        Ok(Some(CustomWaveformOutput { points, colors, additive: cfg.additive, use_dots: cfg.use_dots }))
    }

    /// Fetch and smooth the PCM/spectrum samples (forward + backward IIR), scaled.
    fn smoothed_samples(
        &self,
        state: &PresetState,
        cfg: &crate::state::CustomWaveformConfig,
        count: i32,
        max_count: i32,
    ) -> (Vec<f32>, Vec<f32>) {
        let (pcm_l, pcm_r): (&[f32], &[f32]) = if cfg.spectrum {
            (&state.audio.spectrum_left, &state.audio.spectrum_right)
        } else {
            (&state.audio.waveform_left, &state.audio.waveform_right)
        };

        let mult = cfg.scaling * state.wave_scale * if cfg.spectrum { 0.15 } else { 0.004 };
        let offset1 = if cfg.spectrum { 0 } else { (max_count - count) / 2 - cfg.sep / 2 };
        let offset2 = if cfg.spectrum { 0 } else { (max_count - count) / 2 + cfg.sep / 2 };
        let step = if cfg.spectrum { (max_count - cfg.sep) as f32 / count as f32 } else { 1.0 };
        let mix1 = (cfg.smoothing * 0.98).max(0.0).sqrt();
        let mix2 = 1.0 - mix1;

        let n = count as usize;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        let at = |buf: &[f32], i: i32| -> f32 { buf.get(i.clamp(0, buf.len() as i32 - 1) as usize).copied().unwrap_or(0.0) };

        l[0] = at(pcm_l, offset1);
        r[0] = at(pcm_r, offset2);
        for s in 1..n {
            let idx = (s as f32 * step) as i32;
            l[s] = at(pcm_l, idx + offset1) * mix2 + l[s - 1] * mix1;
            r[s] = at(pcm_r, idx + offset2) * mix2 + r[s - 1] * mix1;
        }
        for s in (0..n.saturating_sub(1)).rev() {
            l[s] = l[s] * mix2 + l[s + 1] * mix1;
            r[s] = r[s] * mix2 + r[s + 1] * mix1;
        }
        for s in 0..n {
            l[s] *= mult;
            r[s] *= mult;
        }
        (l, r)
    }
}

fn modulo(x: f64) -> f32 {
    // Wrap into [0, 256/255), matching Renderer::Color::Modulo.
    let m = 256.0f32 / 255.0;
    let x = x as f32;
    ((x % m) + m) % m
}

/// The 4-point vertex-doubling smooth, carrying per-vertex colors.
fn smooth_wave(points: &[[f32; 2]], colors: &[[f32; 4]]) -> (Vec<[f32; 2]>, Vec<[f32; 4]>) {
    let n = points.len();
    if n < 2 {
        return (points.to_vec(), colors.to_vec());
    }
    const C1: f32 = -0.15;
    const C2: f32 = 1.15;
    const C3: f32 = 1.15;
    const C4: f32 = -0.15;
    const INV: f32 = 1.0 / (C1 + C2 + C3 + C4);

    let mut out_p = vec![[0.0f32; 2]; n * 2 - 1];
    let mut out_c = vec![[0.0f32; 4]; n * 2 - 1];
    let mut below = 0;
    let mut above2 = 1;
    let mut oi = 0;
    for i in 0..n - 1 {
        let above = above2;
        above2 = (i + 2).min(n - 1);
        out_p[oi] = points[i];
        out_c[oi] = colors[i];
        out_c[oi + 1] = colors[i];
        out_p[oi + 1] = [
            (C1 * points[below][0] + C2 * points[i][0] + C3 * points[above][0] + C4 * points[above2][0]) * INV,
            (C1 * points[below][1] + C2 * points[i][1] + C3 * points[above][1] + C4 * points[above2][1]) * INV,
        ];
        below = i;
        oi += 2;
    }
    out_p[oi] = points[n - 1];
    out_c[oi] = colors[n - 1];
    (out_p, out_c)
}

fn compile_opt(code: &str, block: &'static str) -> Result<Option<Program>, PresetError> {
    if code.trim().is_empty() {
        return Ok(None);
    }
    Program::compile(code).map(Some).map_err(|e| PresetError::compile(block, e))
}
