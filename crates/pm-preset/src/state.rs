//! Port of `MilkdropPreset/PresetState.{hpp,cpp}` — per-preset values, defaults,
//! and the code blocks loaded from the file.

use crate::parser::PresetFile;
use pm_audio::FrameAudioData;

pub const Q_VAR_COUNT: usize = 32;
pub const CUSTOM_WAVEFORM_COUNT: usize = 4;
pub const CUSTOM_SHAPE_COUNT: usize = 4;

/// Per-frame *input* values supplied by the host (the subset of projectM's
/// `RenderContext` the equations read). Kept here so `pm-preset` needs no GPU
/// dependency; the orchestrator bridges this from `pm_render::RenderContext`.
#[derive(Debug, Clone)]
pub struct FrameParams {
    pub time: f32,
    pub fps: f32,
    pub frame: i32,
    pub progress: f32,
    pub viewport_width: i32,
    pub viewport_height: i32,
    pub aspect_x: f32,
    pub aspect_y: f32,
    pub inv_aspect_x: f32,
    pub inv_aspect_y: f32,
    pub mesh_x: i32,
    pub mesh_y: i32,
}

impl Default for FrameParams {
    fn default() -> Self {
        FrameParams {
            time: 0.0,
            fps: 60.0,
            frame: 0,
            progress: 0.0,
            viewport_width: 0,
            viewport_height: 0,
            aspect_x: 1.0,
            aspect_y: 1.0,
            inv_aspect_x: 1.0,
            inv_aspect_y: 1.0,
            mesh_x: 64,
            mesh_y: 48,
        }
    }
}

/// All values and code for a single preset. Defaults match Milkdrop's.
#[derive(Debug, Clone)]
pub struct PresetState {
    // General / image
    pub gamma_adj: f32,
    pub video_echo_zoom: f32,
    pub video_echo_alpha: f32,
    pub video_echo_orientation: i32,
    pub decay: f32,
    pub shader: f32,
    pub red_blue_stereo: bool,
    pub brighten: bool,
    pub darken: bool,
    pub solarize: bool,
    pub invert: bool,
    pub blur1_min: f32,
    pub blur2_min: f32,
    pub blur3_min: f32,
    pub blur1_max: f32,
    pub blur2_max: f32,
    pub blur3_max: f32,
    pub blur1_edge_darken: f32,

    // Wave
    pub wave_mode: i32,
    pub additive_waves: bool,
    pub wave_alpha: f32,
    pub wave_scale: f32,
    pub wave_smoothing: f32,
    pub wave_dots: bool,
    pub wave_thick: bool,
    pub wave_param: f32,
    pub mod_wave_alpha_by_volume: bool,
    pub mod_wave_alpha_start: f32,
    pub mod_wave_alpha_end: f32,
    pub maximize_wave_color: bool,
    pub wave_r: f32,
    pub wave_g: f32,
    pub wave_b: f32,
    pub wave_x: f32,
    pub wave_y: f32,

    // Motion
    pub zoom: f32,
    pub rot: f32,
    pub rot_cx: f32,
    pub rot_cy: f32,
    pub x_push: f32,
    pub y_push: f32,
    pub warp_amount: f32,
    pub stretch_x: f32,
    pub stretch_y: f32,
    pub tex_wrap: bool,
    pub darken_center: bool,
    pub warp_anim_speed: f32,
    pub warp_scale: f32,
    pub zoom_exponent: f32,

    // Motion vectors
    pub mv_x: f32,
    pub mv_y: f32,
    pub mv_dx: f32,
    pub mv_dy: f32,
    pub mv_l: f32,
    pub mv_r: f32,
    pub mv_g: f32,
    pub mv_b: f32,
    pub mv_a: f32,

    // Borders
    pub outer_border_size: f32,
    pub outer_border_r: f32,
    pub outer_border_g: f32,
    pub outer_border_b: f32,
    pub outer_border_a: f32,
    pub inner_border_size: f32,
    pub inner_border_r: f32,
    pub inner_border_g: f32,
    pub inner_border_b: f32,
    pub inner_border_a: f32,

    // Versions
    pub preset_version: i32,
    pub warp_shader_version: i32,
    pub composite_shader_version: i32,

    // Code blocks
    pub per_frame_init_code: String,
    pub per_frame_code: String,
    pub per_pixel_code: String,
    pub custom_wave_init_code: [String; CUSTOM_WAVEFORM_COUNT],
    pub custom_wave_per_frame_code: [String; CUSTOM_WAVEFORM_COUNT],
    pub custom_wave_per_point_code: [String; CUSTOM_WAVEFORM_COUNT],
    pub custom_shape_init_code: [String; CUSTOM_SHAPE_COUNT],
    pub custom_shape_per_frame_code: [String; CUSTOM_SHAPE_COUNT],
    pub warp_shader: String,
    pub composite_shader: String,

    // Inter-frame state
    pub frame_q_variables: [f64; Q_VAR_COUNT],

    // Per-frame inputs
    pub frame: FrameParams,
    pub audio: FrameAudioData,
}

impl Default for PresetState {
    fn default() -> Self {
        PresetState {
            gamma_adj: 2.0,
            video_echo_zoom: 2.0,
            video_echo_alpha: 0.0,
            video_echo_orientation: 0,
            decay: 0.98,
            shader: 0.0,
            red_blue_stereo: false,
            brighten: false,
            darken: false,
            solarize: false,
            invert: false,
            blur1_min: 0.0,
            blur2_min: 0.0,
            blur3_min: 0.0,
            blur1_max: 1.0,
            blur2_max: 1.0,
            blur3_max: 1.0,
            blur1_edge_darken: 0.25,

            wave_mode: 0,
            additive_waves: false,
            wave_alpha: 0.8,
            wave_scale: 1.0,
            wave_smoothing: 0.75,
            wave_dots: false,
            wave_thick: false,
            wave_param: 0.0,
            mod_wave_alpha_by_volume: false,
            mod_wave_alpha_start: 0.75,
            mod_wave_alpha_end: 0.95,
            maximize_wave_color: true,
            wave_r: 1.0,
            wave_g: 1.0,
            wave_b: 1.0,
            wave_x: 0.5,
            wave_y: 0.5,

            zoom: 1.0,
            rot: 0.0,
            rot_cx: 0.5,
            rot_cy: 0.5,
            x_push: 0.0,
            y_push: 0.0,
            warp_amount: 1.0,
            stretch_x: 1.0,
            stretch_y: 1.0,
            tex_wrap: true,
            darken_center: false,
            warp_anim_speed: 1.0,
            warp_scale: 1.0,
            zoom_exponent: 1.0,

            mv_x: 12.0,
            mv_y: 9.0,
            mv_dx: 0.0,
            mv_dy: 0.0,
            mv_l: 0.9,
            mv_r: 1.0,
            mv_g: 1.0,
            mv_b: 1.0,
            mv_a: 0.0,

            outer_border_size: 0.01,
            outer_border_r: 0.0,
            outer_border_g: 0.0,
            outer_border_b: 0.0,
            outer_border_a: 0.0,
            inner_border_size: 0.01,
            inner_border_r: 0.25,
            inner_border_g: 0.25,
            inner_border_b: 0.25,
            inner_border_a: 0.0,

            preset_version: 100,
            warp_shader_version: 2,
            composite_shader_version: 2,

            per_frame_init_code: String::new(),
            per_frame_code: String::new(),
            per_pixel_code: String::new(),
            custom_wave_init_code: Default::default(),
            custom_wave_per_frame_code: Default::default(),
            custom_wave_per_point_code: Default::default(),
            custom_shape_init_code: Default::default(),
            custom_shape_per_frame_code: Default::default(),
            warp_shader: String::new(),
            composite_shader: String::new(),

            frame_q_variables: [0.0; Q_VAR_COUNT],

            frame: FrameParams::default(),
            audio: FrameAudioData::default(),
        }
    }
}

impl PresetState {
    /// Load values and code from a parsed `.milk` file, overriding defaults.
    /// Mirrors `PresetState::Initialize`.
    pub fn initialize(file: &PresetFile) -> PresetState {
        let mut s = PresetState::default();

        // General:
        s.decay = file.get_float("fDecay", s.decay);
        s.gamma_adj = file.get_float("fGammaAdj", s.gamma_adj);
        s.video_echo_zoom = file.get_float("fVideoEchoZoom", s.video_echo_zoom);
        s.video_echo_alpha = file.get_float("fVideoEchoAlpha", s.video_echo_alpha);
        s.video_echo_orientation = file.get_int("nVideoEchoOrientation", s.video_echo_orientation);
        s.red_blue_stereo = file.get_bool("bRedBlueStereo", s.red_blue_stereo);
        s.brighten = file.get_bool("bBrighten", s.brighten);
        s.darken = file.get_bool("bDarken", s.darken);
        s.solarize = file.get_bool("bSolarize", s.solarize);
        s.invert = file.get_bool("bInvert", s.invert);
        s.shader = file.get_float("fShader", s.shader);
        s.blur1_min = file.get_float("b1n", s.blur1_min);
        s.blur2_min = file.get_float("b2n", s.blur2_min);
        s.blur3_min = file.get_float("b3n", s.blur3_min);
        s.blur1_max = file.get_float("b1x", s.blur1_max);
        s.blur2_max = file.get_float("b2x", s.blur2_max);
        s.blur3_max = file.get_float("b3x", s.blur3_max);
        s.blur1_edge_darken = file.get_float("b1ed", s.blur1_edge_darken);

        // Wave:
        s.wave_mode = file.get_int("nWaveMode", s.wave_mode);
        s.additive_waves = file.get_bool("bAdditiveWaves", s.additive_waves);
        s.wave_dots = file.get_bool("bWaveDots", s.wave_dots);
        s.wave_thick = file.get_bool("bWaveThick", s.wave_thick);
        s.mod_wave_alpha_by_volume = file.get_bool("bModWaveAlphaByVolume", s.mod_wave_alpha_by_volume);
        s.maximize_wave_color = file.get_bool("bMaximizeWaveColor", s.maximize_wave_color);
        s.wave_alpha = file.get_float("fWaveAlpha", s.wave_alpha);
        s.wave_scale = file.get_float("fWaveScale", s.wave_scale);
        s.wave_smoothing = file.get_float("fWaveSmoothing", s.wave_smoothing);
        s.wave_param = file.get_float("fWaveParam", s.wave_param);
        s.mod_wave_alpha_start = file.get_float("fModWaveAlphaStart", s.mod_wave_alpha_start);
        s.mod_wave_alpha_end = file.get_float("fModWaveAlphaEnd", s.mod_wave_alpha_end);
        s.wave_r = file.get_float("wave_r", s.wave_r);
        s.wave_g = file.get_float("wave_g", s.wave_g);
        s.wave_b = file.get_float("wave_b", s.wave_b);
        s.wave_x = file.get_float("wave_x", s.wave_x);
        s.wave_y = file.get_float("wave_y", s.wave_y);

        // Motion vectors:
        s.mv_x = file.get_float("nMotionVectorsX", s.mv_x);
        s.mv_y = file.get_float("nMotionVectorsY", s.mv_y);
        s.mv_dx = file.get_float("mv_dx", s.mv_dx);
        s.mv_dy = file.get_float("mv_dy", s.mv_dy);
        s.mv_l = file.get_float("mv_l", s.mv_l);
        s.mv_r = file.get_float("mv_r", s.mv_r);
        s.mv_g = file.get_float("mv_g", s.mv_g);
        s.mv_b = file.get_float("mv_b", s.mv_b);
        // Backwards-compat: bMotionVectorsOn enables them, then mv_a overrides.
        s.mv_a = if file.get_bool("bMotionVectorsOn", false) { 1.0 } else { 0.0 };
        s.mv_a = file.get_float("mv_a", s.mv_a);

        // Motion:
        s.zoom = file.get_float("zoom", s.zoom);
        s.rot = file.get_float("rot", s.rot);
        s.rot_cx = file.get_float("cx", s.rot_cx);
        s.rot_cy = file.get_float("cy", s.rot_cy);
        s.x_push = file.get_float("dx", s.x_push);
        s.y_push = file.get_float("dy", s.y_push);
        s.warp_amount = file.get_float("warp", s.warp_amount);
        s.stretch_x = file.get_float("sx", s.stretch_x);
        s.stretch_y = file.get_float("sy", s.stretch_y);
        s.tex_wrap = file.get_bool("bTexWrap", s.tex_wrap);
        s.darken_center = file.get_bool("bDarkenCenter", s.darken_center);
        s.warp_anim_speed = file.get_float("fWarpAnimSpeed", s.warp_anim_speed);
        s.warp_scale = file.get_float("fWarpScale", s.warp_scale);
        s.zoom_exponent = file.get_float("fZoomExponent", s.zoom_exponent);

        // Borders:
        s.outer_border_size = file.get_float("ob_size", s.outer_border_size);
        s.outer_border_r = file.get_float("ob_r", s.outer_border_r);
        s.outer_border_g = file.get_float("ob_g", s.outer_border_g);
        s.outer_border_b = file.get_float("ob_b", s.outer_border_b);
        s.outer_border_a = file.get_float("ob_a", s.outer_border_a);
        s.inner_border_size = file.get_float("ib_size", s.inner_border_size);
        s.inner_border_r = file.get_float("ib_r", s.inner_border_r);
        s.inner_border_g = file.get_float("ib_g", s.inner_border_g);
        s.inner_border_b = file.get_float("ib_b", s.inner_border_b);
        s.inner_border_a = file.get_float("ib_a", s.inner_border_a);

        // Versions:
        s.preset_version = file.get_int("MILKDROP_PRESET_VERSION", s.preset_version);
        if s.preset_version < 200 {
            s.warp_shader_version = 0;
            s.composite_shader_version = 0;
        } else if s.preset_version == 200 {
            s.warp_shader_version = file.get_int("PSVERSION", s.warp_shader_version);
            s.composite_shader_version = file.get_int("PSVERSION", s.composite_shader_version);
        } else {
            s.warp_shader_version = file.get_int("PSVERSION_WARP", s.warp_shader_version);
            s.composite_shader_version = file.get_int("PSVERSION_COMP", s.composite_shader_version);
        }

        // Code:
        s.per_frame_init_code = file.get_code("per_frame_init_");
        s.per_frame_code = file.get_code("per_frame_");
        s.per_pixel_code = file.get_code("per_pixel_");

        for i in 0..CUSTOM_WAVEFORM_COUNT {
            let prefix = format!("wave_{i}_");
            s.custom_wave_init_code[i] = file.get_code(&format!("{prefix}init"));
            s.custom_wave_per_frame_code[i] = file.get_code(&format!("{prefix}per_frame"));
            s.custom_wave_per_point_code[i] = file.get_code(&format!("{prefix}per_point"));
        }
        for i in 0..CUSTOM_SHAPE_COUNT {
            let prefix = format!("shape_{i}_");
            s.custom_shape_init_code[i] = file.get_code(&format!("{prefix}init"));
            s.custom_shape_per_frame_code[i] = file.get_code(&format!("{prefix}per_frame"));
        }

        s.warp_shader = file.get_code("warp_");
        s.composite_shader = file.get_code("comp_");

        s
    }
}
