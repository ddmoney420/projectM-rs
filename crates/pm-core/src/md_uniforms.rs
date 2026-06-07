//! The Milkdrop preset-shader uniform block, filled from preset state each
//! frame. Port of `MilkdropShader::LoadVariables`.
//!
//! The memory layout **must** match the `MdUniforms` WGSL struct emitted by
//! `pm_preset::preset_shader` (field order: `c0..c13`, `qa..qh`, `rand_frame`,
//! `rand_preset`, then the 24 `rot_*` matrices as `mat3x4<f32>`).

use bytemuck::{Pod, Zeroable};
use pm_preset::PresetState;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct MdUniforms {
    /// `_c0` .. `_c13`.
    c: [[f32; 4]; 14],
    /// `_qa` .. `_qh` (q1..q32).
    q: [[f32; 4]; 8],
    rand_frame: [f32; 4],
    rand_preset: [f32; 4],
    /// 24 `rot_*` matrices as `mat3x4<f32>` (3 columns of vec4).
    rot: [[[f32; 4]; 3]; 24],
}

fn log2(x: f32) -> f32 {
    x.max(1.0).ln() / std::f32::consts::LN_2
}

impl MdUniforms {
    /// Compute the uniform values for the current frame.
    pub fn from_state(state: &PresetState, time: f32) -> Self {
        let f = &state.frame;
        let a = &state.audio;

        let vp_w = f.viewport_width.max(1) as f32;
        let vp_h = f.viewport_height.max(1) as f32;

        let time_wrapped = time - (time / 10000.0).floor() * 10000.0;
        let mip_x = log2(vp_w);
        let mip_y = log2(vp_h);
        let mip_avg = 0.5 * (mip_x + mip_y);

        let (b1n, b1x) = (state.blur1_min, state.blur1_max);
        let (b2n, b2x) = (state.blur2_min, state.blur2_max);
        let (b3n, b3x) = (state.blur3_min, state.blur3_max);

        // The animated "roam" oscillators (phases from MilkdropShader.cpp).
        let roam = |s: [f32; 4], ph: [f32; 4], sinf: bool| {
            let mut v = [0.0f32; 4];
            for i in 0..4 {
                let arg = time * s[i] + ph[i];
                v[i] = 0.5 + 0.5 * if sinf { arg.sin() } else { arg.cos() };
            }
            v
        };
        let fast_speed = [0.329, 1.293, 5.070, 20.051];
        let fast_phase = [1.2, 3.9, 2.5, 5.4];
        let slow_speed = [0.0050, 0.0085, 0.0133, 0.0217];
        let slow_phase = [2.7, 5.3, 4.5, 3.8];

        let mut c = [[0.0f32; 4]; 14];
        c[0] = [f.aspect_x, f.aspect_y, f.inv_aspect_x, f.inv_aspect_y];
        c[1] = [0.0, 0.0, 0.0, 0.0];
        c[2] = [time_wrapped, f.fps, f.frame as f32, f.progress];
        c[3] = [a.bass, a.mid, a.treb, a.vol];
        c[4] = [a.bass_att, a.mid_att, a.treb_att, a.vol_att];
        c[5] = [b1x - b1n, b1n, b2x - b2n, b2n];
        c[6] = [b3x - b3n, b3n, b1n, b1x];
        c[7] = [vp_w, vp_h, 1.0 / vp_w, 1.0 / vp_h];
        c[8] = roam(fast_speed, fast_phase, false);
        c[9] = roam(fast_speed, fast_phase, true);
        c[10] = roam(slow_speed, slow_phase, false);
        c[11] = roam(slow_speed, slow_phase, true);
        c[12] = [mip_x, mip_y, mip_avg, 0.0];
        c[13] = [b2n, b2x, b3n, b3x];

        // q1..q32 packed into 8 vec4.
        let mut q = [[0.0f32; 4]; 8];
        for (i, bank) in q.iter_mut().enumerate() {
            for (j, slot) in bank.iter_mut().enumerate() {
                *slot = state.frame_q_variables[i * 4 + j] as f32;
            }
        }

        // Per-frame / per-preset randoms (deterministic stand-ins for now).
        let rand_frame = roam([0.7, 1.1, 1.7, 2.3], [0.0, 1.0, 2.0, 3.0], true);
        let rand_preset = state.hue_random_offsets;

        // Rotation matrices: identity (mat3x4 columns). 3D-rotation presets are
        // approximate until the full rot_* setup is ported.
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let rot = [identity; 24];

        MdUniforms { c, q, rand_frame, rand_preset, rot }
    }
}
