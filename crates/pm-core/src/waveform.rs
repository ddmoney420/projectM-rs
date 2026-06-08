//! Port of `MilkdropPreset/Waveforms/` — generates the audio waveform geometry
//! drawn into the feedback buffer each frame.
//!
//! Ports the shared `WaveformMath` scaffolding (scale, IIR smoothing, aspect,
//! mystery param, the 4-point `SmoothWave`) and the standard `nWaveMode` family:
//! Circle (0), XY-oscillation spiral (1), centered spiro (2/3), derivative line
//! (4), explosive hash (5), Line (6) and double line (7). Spectrum line (8) and
//! the Milkdrop2077 modes (9–15) fall back to Line.
//!
//! `1.57`/`2.3`/`0.3`/`6.28` are the exact literals Milkdrop uses (not `PI/2`,
//! `TAU`, …); kept verbatim for parity.
#![allow(clippy::approx_constant)]

use pm_audio::WAVEFORM_SAMPLES;
use pm_preset::PresetState;

/// Scratch length of the scaled PCM buffers — matches Milkdrop's
/// `WaveformMaxPoints` so modes that index `pcm[i + 32]` past the
/// [`WAVEFORM_SAMPLES`] valid range stay in bounds (the tail reads as 0).
const PCM_SCRATCH: usize = 512;

/// A generated waveform: one or two polylines in clip space, and whether the
/// primary line is a closed loop.
pub struct WaveformGeometry {
    pub points: Vec<[f32; 2]>,
    /// Second polyline for the double-line mode (mode 7).
    pub points2: Option<Vec<[f32; 2]>>,
    pub is_loop: bool,
}

/// Build the waveform geometry for the current preset state (call after the
/// per-frame code has run, so `wave_*` reflect this frame).
pub fn generate(state: &PresetState) -> WaveformGeometry {
    let mode = state.wave_mode.rem_euclid(16);
    let is_loop = matches!(mode, 0 | 1); // Circle, XYOscillationSpiral
    let uses_normalized = matches!(mode, 0 | 1 | 4); // + DerivativeLine

    let (pcm_l, pcm_r) = scaled_pcm(state);

    // Aspect multipliers (only the larger axis is scaled down).
    let (vp_x, vp_y) = (state.frame.viewport_width, state.frame.viewport_height);
    let (aspect_x, aspect_y) = if vp_x > vp_y {
        (1.0, vp_y as f32 / vp_x.max(1) as f32)
    } else {
        (vp_x as f32 / vp_y.max(1) as f32, 1.0)
    };

    let mut mystery = state.wave_param;
    if uses_normalized && !(-1.0..=1.0).contains(&mystery) {
        mystery = mystery * 0.5 + 0.5;
        mystery -= mystery.floor();
        mystery = mystery.abs() * 2.0 - 1.0;
    }

    let wave_x = 2.0 * state.wave_x - 1.0;
    let wave_y = 2.0 * state.wave_y - 1.0;

    let ctx = Ctx { state, pcm_l: &pcm_l, pcm_r: &pcm_r, aspect_x, aspect_y, mystery, wave_x, wave_y };

    let (raw1, raw2) = match mode {
        0 => (circle_vertices(&ctx), None),
        1 => (xy_spiral_vertices(&ctx), None),
        2 | 3 => (centered_spiro_vertices(&ctx), None),
        4 => (derivative_line_vertices(&ctx), None),
        5 => (explosive_hash_vertices(&ctx), None),
        7 => {
            let (a, b) = double_line_vertices(&ctx);
            (a, Some(b))
        }
        _ => (line_vertices(&ctx), None), // 6 (Line) + 8..15 fallback
    };

    WaveformGeometry { points: smooth_wave(&raw1), points2: raw2.map(|w| smooth_wave(&w)), is_loop }
}

/// Shared inputs for a mode's vertex generator.
struct Ctx<'a> {
    state: &'a PresetState,
    pcm_l: &'a [f32],
    pcm_r: &'a [f32],
    aspect_x: f32,
    aspect_y: f32,
    mystery: f32,
    wave_x: f32,
    wave_y: f32,
}

/// Scale by `waveScale/128` and apply the IIR smoothing filter (alpha =
/// `waveSmoothing`). The buffers keep the full analysis length so modes that
/// index `pcm[i + 32]` past [`WAVEFORM_SAMPLES`] stay in bounds.
fn scaled_pcm(state: &PresetState) -> (Vec<f32>, Vec<f32>) {
    // Copy the valid waveform samples into a fixed scratch, zero-padding the
    // tail (Milkdrop leaves stale values there; zero is the deterministic port).
    let mut l = vec![0.0f32; PCM_SCRATCH];
    let mut r = vec![0.0f32; PCM_SCRATCH];
    l[..WAVEFORM_SAMPLES].copy_from_slice(&state.audio.waveform_left[..WAVEFORM_SAMPLES]);
    r[..WAVEFORM_SAMPLES].copy_from_slice(&state.audio.waveform_right[..WAVEFORM_SAMPLES]);

    let scale = state.wave_scale / 128.0;
    l[0] *= scale;
    r[0] *= scale;
    let mix2 = state.wave_smoothing;
    let mix1 = scale * (1.0 - mix2);
    for i in 1..PCM_SCRATCH {
        l[i] = l[i] * mix1 + l[i - 1] * mix2;
        r[i] = r[i] * mix1 + r[i - 1] * mix2;
    }
    (l, r)
}

/// Mode 0: circular wave (closed loop), `Circle::GenerateVertices`.
fn circle_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let samples = WAVEFORM_SAMPLES / 2;
    let sample_offset = (WAVEFORM_SAMPLES - samples) / 2;
    let inv = 1.0 / samples as f32;
    let time = c.state.frame.time;

    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let mut radius = 0.5 + 0.4 * c.pcm_r[i + sample_offset] + c.mystery;
        let angle = i as f32 * inv * 6.28 + time * 0.2;
        if i < samples / 10 {
            let mut mix = i as f32 / (samples as f32 * 0.1);
            mix = 0.5 - 0.5 * (mix * std::f32::consts::PI).cos();
            let radius2 = 0.5 + 0.4 * c.pcm_r[i + samples + sample_offset] + c.mystery;
            radius = radius2 * (1.0 - mix) + radius * mix;
        }
        out.push([radius * angle.cos() * c.aspect_y + c.wave_x, radius * angle.sin() * c.aspect_x + c.wave_y]);
    }
    out
}

/// Mode 1: X-Y oscillation that spirals over time, `XYOscillationSpiral`.
fn xy_spiral_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let samples = WAVEFORM_SAMPLES / 2;
    let time = c.state.frame.time;
    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let radius = 0.53 + 0.43 * c.pcm_r[i] + c.mystery;
        let angle = c.pcm_l[i + 32] * 1.57 + time * 2.3;
        out.push([radius * angle.cos() * c.aspect_y + c.wave_x, radius * angle.sin() * c.aspect_x + c.wave_y]);
    }
    out
}

/// Modes 2 & 3: centered spiro (the alpha difference is handled in colouring),
/// `CenteredSpiro::GenerateVertices`.
fn centered_spiro_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let samples = WAVEFORM_SAMPLES;
    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        out.push([c.pcm_r[i] * c.aspect_y + c.wave_x, c.pcm_l[i + 32] * c.aspect_x + c.wave_y]);
    }
    out
}

/// Mode 4: horizontal "script" of the left channel with momentum smoothing,
/// `DerivativeLine::GenerateVertices`.
fn derivative_line_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let mut samples = WAVEFORM_SAMPLES;
    if samples > c.state.frame.viewport_width as usize / 3 {
        samples /= 3;
    }
    let sample_offset = (WAVEFORM_SAMPLES - samples) / 2;
    let w1 = 0.45 + 0.5 * (c.mystery * 0.5 + 0.5);
    let w2 = 1.0 - w1;
    let inv = 1.0 / samples as f32;

    let mut out: Vec<[f32; 2]> = Vec::with_capacity(samples);
    for i in 0..samples {
        let x = -1.0 + 2.0 * (i as f32 * inv) + c.wave_x + c.pcm_r[i + 25 + sample_offset] * 0.44;
        let y = c.pcm_l[i + sample_offset] * 0.47 + c.wave_y;
        if i > 1 {
            // Momentum: extrapolate from the previous two points.
            out.push([
                x * w2 + w1 * (out[i - 1][0] * 2.0 - out[i - 2][0]),
                y * w2 + w1 * (out[i - 1][1] * 2.0 - out[i - 2][1]),
            ]);
        } else {
            out.push([x, y]);
        }
    }
    out
}

/// Mode 5: "explosive hash" complex-number thingy, `ExplosiveHash`.
fn explosive_hash_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let samples = WAVEFORM_SAMPLES;
    let time = c.state.frame.time;
    let cos_r = (time * 0.3).cos();
    let sin_r = (time * 0.3).sin();
    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let x0 = c.pcm_r[i] * c.pcm_l[i + 32] + c.pcm_l[i] * c.pcm_r[i + 32];
        let y0 = c.pcm_r[i] * c.pcm_r[i] - c.pcm_l[i + 32] * c.pcm_l[i + 32];
        out.push([
            (x0 * cos_r - y0 * sin_r) * c.aspect_y + c.wave_x,
            (x0 * sin_r + y0 * cos_r) * c.aspect_x + c.wave_y,
        ]);
    }
    out
}

/// Result of `LineBase::ClipWaveformEdges`: the clipped start edge, per-sample
/// step, perpendicular direction, and waveform sample offset.
struct Clip {
    edge_x: f32,
    edge_y: f32,
    distance_x: f32,
    distance_y: f32,
    perp_dx: f32,
    perp_dy: f32,
    sample_offset: usize,
}

/// Port of `LineBase::ClipWaveformEdges`: builds the line endpoints from the
/// angle and clips them to the (slightly over-sized) screen rectangle.
fn clip_waveform_edges(samples: usize, wave_x: f32, angle: f32) -> Clip {
    let dx = angle.cos();
    let dy = angle.sin();
    let mut edge_x = [wave_x * (angle + 1.57).cos() - dx * 3.0, wave_x * (angle + 1.57).cos() + dx * 3.0];
    let mut edge_y = [wave_x * (angle + 1.57).sin() - dy * 3.0, wave_x * (angle + 1.57).sin() + dy * 3.0];

    for i in 0..2 {
        let other = 1 - i;
        for j in 0..4 {
            let (over, on_x) = match j {
                0 => (edge_x[i] > 1.1, true),
                1 => (edge_x[i] < -1.1, true),
                2 => (edge_y[i] > 1.1, false),
                _ => (edge_y[i] < -1.1, false),
            };
            if !over {
                continue;
            }
            let bound = match j {
                0 | 2 => 1.1,
                _ => -1.1,
            };
            let t = if on_x {
                (bound - edge_x[other]) / (edge_x[i] - edge_x[other])
            } else {
                (bound - edge_y[other]) / (edge_y[i] - edge_y[other])
            };
            let diff_x = edge_x[i] - edge_x[other];
            let diff_y = edge_y[i] - edge_y[other];
            edge_x[i] = edge_x[other] + diff_x * t;
            edge_y[i] = edge_y[other] + diff_y * t;
        }
    }

    let sample_offset = (WAVEFORM_SAMPLES - samples) / 2;
    let distance_x = (edge_x[1] - edge_x[0]) / samples as f32;
    let distance_y = (edge_y[1] - edge_y[0]) / samples as f32;
    let angle2 = distance_y.atan2(distance_x);
    Clip {
        edge_x: edge_x[0],
        edge_y: edge_y[0],
        distance_x,
        distance_y,
        perp_dx: (angle2 + 1.57).cos(),
        perp_dy: (angle2 + 1.57).sin(),
        sample_offset,
    }
}

/// Number of line samples for the Line/DoubleLine modes (thinned on narrow
/// viewports, matching `Line::GenerateVertices`).
fn line_samples(viewport_width: i32) -> usize {
    let mut samples = WAVEFORM_SAMPLES / 2;
    if samples > viewport_width as usize / 3 {
        samples /= 3;
    }
    samples
}

/// Mode 6: angle-adjustable left-channel line, `Line::GenerateVertices`.
fn line_vertices(c: &Ctx) -> Vec<[f32; 2]> {
    let samples = line_samples(c.state.frame.viewport_width);
    let clip = clip_waveform_edges(samples, c.wave_x, 1.57 * c.mystery);
    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let p = 0.25 * c.pcm_l[i + clip.sample_offset];
        out.push([
            clip.edge_x + clip.distance_x * i as f32 + clip.perp_dx * p,
            clip.edge_y + clip.distance_y * i as f32 + clip.perp_dy * p,
        ]);
    }
    out
}

/// Mode 7: two channels shown as separated lines, `DoubleLine::GenerateVertices`.
fn double_line_vertices(c: &Ctx) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let samples = line_samples(c.state.frame.viewport_width);
    let clip = clip_waveform_edges(samples, c.wave_x, 1.57 * c.mystery);
    let separation = (c.wave_y * 0.5 + 0.5).powi(2);

    let mut w1 = Vec::with_capacity(samples);
    let mut w2 = Vec::with_capacity(samples);
    for i in 0..samples {
        let base_x = clip.edge_x + clip.distance_x * i as f32;
        let base_y = clip.edge_y + clip.distance_y * i as f32;
        let pl = 0.25 * c.pcm_l[i + clip.sample_offset] + separation;
        let pr = 0.25 * c.pcm_r[i + clip.sample_offset] - separation;
        w1.push([base_x + clip.perp_dx * pl, base_y + clip.perp_dy * pl]);
        w2.push([base_x + clip.perp_dx * pr, base_y + clip.perp_dy * pr]);
    }
    (w1, w2)
}

/// The 4-point smoothing that interpolates an extra vertex between each pair,
/// doubling the resolution (`WaveformMath::SmoothWave`).
fn smooth_wave(input: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if input.is_empty() {
        return Vec::new();
    }
    const C1: f32 = -0.15;
    const C2: f32 = 1.15;
    const C3: f32 = 1.15;
    const C4: f32 = -0.15;
    const INV: f32 = 1.0 / (C1 + C2 + C3 + C4);

    let n = input.len();
    let mut out = vec![[0.0f32; 2]; n * 2 - 1];
    let mut index_below = 0;
    let mut index_above2 = 1.min(n - 1);
    let mut oi = 0;
    for i in 0..n - 1 {
        let index_above = index_above2;
        index_above2 = (i + 2).min(n - 1);
        out[oi] = input[i];
        out[oi + 1] = [
            (C1 * input[index_below][0] + C2 * input[i][0] + C3 * input[index_above][0] + C4 * input[index_above2][0]) * INV,
            (C1 * input[index_below][1] + C2 * input[i][1] + C3 * input[index_above][1] + C4 * input[index_above2][1]) * INV,
        ];
        index_below = i;
        oi += 2;
    }
    out[oi] = input[n - 1];
    out
}
