//! Port of `MilkdropPreset/Waveforms/` — generates the audio waveform geometry
//! drawn into the feedback buffer each frame.
//!
//! Ports the shared `WaveformMath` scaffolding (scale, IIR smoothing, aspect,
//! mystery param) and two of the most common modes: **Circle** (mode 0, the
//! default) and **Line** (mode 6). Other modes currently fall back to the
//! nearest of these.

use pm_preset::PresetState;

const WAVEFORM_SAMPLES: usize = pm_audio::WAVEFORM_SAMPLES;

/// A generated waveform: clip-space points and whether it's a closed loop.
pub struct WaveformGeometry {
    pub points: Vec<[f32; 2]>,
    pub is_loop: bool,
}

/// Build the waveform geometry for the current preset state (call after the
/// per-frame code has run, so `wave_*` reflect this frame).
pub fn generate(state: &PresetState) -> WaveformGeometry {
    let mode = state.wave_mode.rem_euclid(16);
    let is_circle = mode == 0;
    let is_loop = is_circle;

    let (pcm_l, pcm_r) = scaled_pcm(state);

    // Aspect multipliers (only the larger axis is scaled down).
    let (vp_x, vp_y) = (state.frame.viewport_width, state.frame.viewport_height);
    let (aspect_x, aspect_y) = if vp_x > vp_y {
        (1.0, vp_y as f32 / vp_x.max(1) as f32)
    } else {
        (vp_x as f32 / vp_y.max(1) as f32, 1.0)
    };

    let uses_normalized = is_circle;
    let mut mystery = state.wave_param;
    if uses_normalized && !(-1.0..=1.0).contains(&mystery) {
        mystery = mystery * 0.5 + 0.5;
        mystery -= mystery.floor();
        mystery = mystery.abs() * 2.0 - 1.0;
    }

    let wave_x = 2.0 * state.wave_x - 1.0;
    let wave_y = 2.0 * state.wave_y - 1.0;

    let raw = if is_circle {
        circle_vertices(state, &pcm_r, aspect_x, aspect_y, mystery, wave_x, wave_y)
    } else {
        line_vertices(state, &pcm_l, mystery, wave_x)
    };

    let points = smooth_wave(&raw);
    WaveformGeometry { points, is_loop }
}

/// Scale by `waveScale/128` and apply the IIR smoothing filter (alpha =
/// `waveSmoothing`) to the left/right waveform samples.
fn scaled_pcm(state: &PresetState) -> (Vec<f32>, Vec<f32>) {
    let mut l: Vec<f32> = state.audio.waveform_left[..WAVEFORM_SAMPLES].to_vec();
    let mut r: Vec<f32> = state.audio.waveform_right[..WAVEFORM_SAMPLES].to_vec();

    let scale = state.wave_scale / 128.0;
    l[0] *= scale;
    r[0] *= scale;
    let mix2 = state.wave_smoothing;
    let mix1 = scale * (1.0 - mix2);
    for i in 1..WAVEFORM_SAMPLES {
        l[i] = l[i] * mix1 + l[i - 1] * mix2;
        r[i] = r[i] * mix1 + r[i - 1] * mix2;
    }
    (l, r)
}

// `6.28` is the exact literal Milkdrop uses here (not `TAU`); keeping it for parity.
#[allow(clippy::approx_constant)]
fn circle_vertices(
    state: &PresetState,
    pcm_r: &[f32],
    aspect_x: f32,
    aspect_y: f32,
    mystery: f32,
    wave_x: f32,
    wave_y: f32,
) -> Vec<[f32; 2]> {
    let samples = WAVEFORM_SAMPLES / 2;
    let sample_offset = (WAVEFORM_SAMPLES - samples) / 2;
    let inv = 1.0 / samples as f32;
    let time = state.frame.time;

    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let mut radius = 0.5 + 0.4 * pcm_r[i + sample_offset] + mystery;
        let angle = i as f32 * inv * 6.28 + time * 0.2;
        if i < samples / 10 {
            let mut mix = i as f32 / (samples as f32 * 0.1);
            mix = 0.5 - 0.5 * (mix * std::f32::consts::PI).cos();
            let radius2 = 0.5 + 0.4 * pcm_r[i + samples + sample_offset] + mystery;
            radius = radius2 * (1.0 - mix) + radius * mix;
        }
        out.push([radius * angle.cos() * aspect_y + wave_x, radius * angle.sin() * aspect_x + wave_y]);
    }
    out
}

fn line_vertices(state: &PresetState, pcm_l: &[f32], mystery: f32, wave_x: f32) -> Vec<[f32; 2]> {
    let mut samples = WAVEFORM_SAMPLES / 2;
    if samples > state.frame.viewport_width as usize / 3 {
        samples /= 3;
    }

    // ClipWaveformEdges: build the line endpoints from the angle, clip to screen.
    let angle = 1.57 * mystery;
    let dx = angle.cos();
    let dy = angle.sin();
    let mut edge_x = [
        wave_x * (angle + 1.57).cos() - dx * 3.0,
        wave_x * (angle + 1.57).cos() + dx * 3.0,
    ];
    let mut edge_y = [
        wave_x * (angle + 1.57).sin() - dy * 3.0,
        wave_x * (angle + 1.57).sin() + dy * 3.0,
    ];
    for i in 0..2 {
        let other = 1 - i;
        for j in 0..4 {
            let (val, lo) = match j {
                0 => (edge_x[i] > 1.1, true),
                1 => (edge_x[i] < -1.1, true),
                2 => (edge_y[i] > 1.1, false),
                _ => (edge_y[i] < -1.1, false),
            };
            if !val {
                continue;
            }
            let bound = match j {
                0 => 1.1,
                1 => -1.1,
                2 => 1.1,
                _ => -1.1,
            };
            let t = if lo {
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
    let dist_x = (edge_x[1] - edge_x[0]) / samples as f32;
    let dist_y = (edge_y[1] - edge_y[0]) / samples as f32;
    let angle2 = dist_y.atan2(dist_x);
    let perp_dx = (angle2 + 1.57).cos();
    let perp_dy = (angle2 + 1.57).sin();

    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let p = 0.25 * pcm_l[i + sample_offset];
        out.push([
            edge_x[0] + dist_x * i as f32 + perp_dx * p,
            edge_y[0] + dist_y * i as f32 + perp_dy * p,
        ]);
    }
    out
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
