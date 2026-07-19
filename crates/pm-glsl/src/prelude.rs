//! The shared GLSL prelude (uniform block + `iChannel` samplers) and the Rust
//! mirror of the uniform block. Both authoring modes are prefixed with
//! [`PRELUDE`], so every compiled shader shares one binding layout.
//!
//! # Binding contract (group 0)
//! - `binding 0` — uniform block `PmUniforms` (see [`ShaderUniforms`]).
//! - `binding 1` — `iChannel0`: the **audio texture** (see below).
//! - `binding 2..=4` — `iChannel1..3`: 1×1 placeholder textures for now.
//!   Arbitrary Shadertoy asset loading is **not** implemented yet.
//! - The samplers naga emits for the above get their own bindings; the actual
//!   numbers are read back from the generated WGSL when building the pipeline.
//!
//! # Audio texture layout (`iChannel0`, [`AUDIO_TEX_WIDTH`]×[`AUDIO_TEX_HEIGHT`])
//! `R8Unorm`, filterable. Row `y=0.25` (top) = **spectrum/FFT**, row `y=0.75`
//! (bottom) = **waveform**, matching Shadertoy convention. Values are in `[0,1]`:
//! spectrum is the projectM `spectrum_left` clamped/scaled; waveform is
//! `waveform_left` mapped `0.5 + x/256`. Sample `texture(iChannel0, vec2(u, 0.25)).x`
//! for FFT and `..0.75).x` for the waveform. This is projectM-derived data — not
//! the browser `AnalyserNode`.

/// Audio texture width (samples per row).
pub const AUDIO_TEX_WIDTH: u32 = 512;
/// Audio texture height (row 0 = spectrum, row 1 = waveform).
pub const AUDIO_TEX_HEIGHT: u32 = 2;

/// Shared GLSL prelude prepended to every user shader (no trailing newline).
///
/// naga's GLSL frontend wants Vulkan-style separated textures + samplers, so
/// `iChannelN` are `#define`d to the `sampler2D(tex, smp)` combined form that
/// Shadertoy's `texture(iChannelN, uv)` calls expand into. One shared sampler
/// serves all four channels. Bindings (group 0): 0=uniforms, 1..4=channel
/// textures, 5=sampler.
pub const PRELUDE: &str = "\
#version 450

layout(std140, set = 0, binding = 0)
uniform PmUniforms {
    vec3 iResolution;
    float iTime;
    vec4 iMouse;
    vec4 iDate;
    float iTimeDelta;
    float iFrame;
    float iSampleRate;
    float pm_pad0;
    vec4 iChannelResolution[4];
    float iBass;
    float iMid;
    float iTreb;
    float iVol;
    float iBassAtt;
    float iMidAtt;
    float iTrebAtt;
    float iVolAtt;
    float iBPM;
    float iBeatPhase;
    float iBeatPulse;
    float iBeatIndex;
    float iBarPhase;
    float iTempoConfidence;
    float pm_pad1;
    float pm_pad2;
};

layout(set = 0, binding = 1) uniform texture2D pm_ch0_tex;
layout(set = 0, binding = 2) uniform texture2D pm_ch1_tex;
layout(set = 0, binding = 3) uniform texture2D pm_ch2_tex;
layout(set = 0, binding = 4) uniform texture2D pm_ch3_tex;
layout(set = 0, binding = 5) uniform sampler pm_smp;

layout(std140, set = 0, binding = 6)
uniform PmUserControls {
    vec4 pm_user[16];
};

#define iChannel0 sampler2D(pm_ch0_tex, pm_smp)
#define iChannel1 sampler2D(pm_ch1_tex, pm_smp)
#define iChannel2 sampler2D(pm_ch2_tex, pm_smp)
#define iChannel3 sampler2D(pm_ch3_tex, pm_smp)";

/// Rust mirror of the `PmUniforms` std140 block. Field order and padding match
/// the GLSL layout exactly (all members are `f32`/`f32` arrays, so `repr(C)`
/// offsets equal std140 offsets). Total size 160 bytes (a multiple of 16).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShaderUniforms {
    pub i_resolution: [f32; 3], // offset 0
    pub i_time: f32,            // 12
    pub i_mouse: [f32; 4],      // 16
    pub i_date: [f32; 4],       // 32
    pub i_time_delta: f32,      // 48
    pub i_frame: f32,           // 52
    pub i_sample_rate: f32,     // 56
    pub pm_pad0: f32,           // 60
    pub i_channel_resolution: [[f32; 4]; 4], // 64..128
    pub i_bass: f32,            // 128
    pub i_mid: f32,             // 132
    pub i_treb: f32,            // 136
    pub i_vol: f32,             // 140
    pub i_bass_att: f32,        // 144
    pub i_mid_att: f32,         // 148
    pub i_treb_att: f32,        // 152
    pub i_vol_att: f32,         // 156
    pub i_bpm: f32,             // 160
    pub i_beat_phase: f32,      // 164
    pub i_beat_pulse: f32,      // 168
    pub i_beat_index: f32,      // 172
    pub i_bar_phase: f32,       // 176
    pub i_tempo_confidence: f32, // 180
    pub pm_pad1: f32,           // 184
    pub pm_pad2: f32,           // 188
}

impl Default for ShaderUniforms {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

// Compile-time check that the layout is 192 bytes (12 × 16), std140-compatible.
const _: () = assert!(std::mem::size_of::<ShaderUniforms>() == 192);
