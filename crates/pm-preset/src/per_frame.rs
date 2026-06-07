//! Port of `MilkdropPreset/PerFrameContext.{hpp,cpp}`.
//!
//! Binds the preset's `per_frame_init` / `per_frame` equations (compiled by
//! [`pm_eval`]) to Milkdrop's named variable model: load state → variables,
//! run the code, read the modified variables back into the state.

use crate::error::PresetError;
use crate::state::{PresetState, Q_VAR_COUNT};
use pm_eval::{Context, Program};

pub struct PerFrameContext {
    ctx: Context,
    init_program: Option<Program>,
    frame_program: Option<Program>,
    /// q1..q32 captured after the init code ran (reset each frame before exec).
    q_after_init: [f64; Q_VAR_COUNT],
}

impl PerFrameContext {
    /// Compile the per-frame init and per-frame code from the preset state.
    pub fn new(state: &PresetState) -> Result<Self, PresetError> {
        let init_program = compile_opt(&state.per_frame_init_code, "per_frame_init")?;
        let frame_program = compile_opt(&state.per_frame_code, "per_frame")?;
        Ok(PerFrameContext {
            ctx: Context::new(),
            init_program,
            frame_program,
            q_after_init: [0.0; Q_VAR_COUNT],
        })
    }

    /// Run the init code once, capturing the resulting q1..q32 values.
    pub fn evaluate_init(&mut self, state: &mut PresetState) -> Result<(), PresetError> {
        if let Some(prog) = &self.init_program {
            prog.run(&mut self.ctx).map_err(|e| PresetError::eval("per_frame_init", e))?;
        }
        for q in 0..Q_VAR_COUNT {
            let v = self.ctx.get(&format!("q{}", q + 1));
            self.q_after_init[q] = v;
            state.frame_q_variables[q] = v;
        }
        Ok(())
    }

    /// Load state into variables, run the per-frame code, and read results back.
    pub fn execute(&mut self, state: &mut PresetState) -> Result<(), PresetError> {
        self.load_state_variables(state);
        if let Some(prog) = &self.frame_program {
            prog.run(&mut self.ctx).map_err(|e| PresetError::eval("per_frame", e))?;
        }
        self.read_back(state);
        Ok(())
    }

    /// Snapshot of q1..q32 after the last per-frame execution (for per-pixel /
    /// shaders to consume).
    pub fn q_variables(&self) -> [f64; Q_VAR_COUNT] {
        let mut q = [0.0; Q_VAR_COUNT];
        for (i, slot) in q.iter_mut().enumerate() {
            *slot = self.ctx.get(&format!("q{}", i + 1));
        }
        q
    }

    fn load_state_variables(&mut self, state: &PresetState) {
        let c = &mut self.ctx;
        // Motion
        c.set("zoom", state.zoom as f64);
        c.set("zoomexp", state.zoom_exponent as f64);
        c.set("rot", state.rot as f64);
        c.set("warp", state.warp_amount as f64);
        c.set("cx", state.rot_cx as f64);
        c.set("cy", state.rot_cy as f64);
        c.set("dx", state.x_push as f64);
        c.set("dy", state.y_push as f64);
        c.set("sx", state.stretch_x as f64);
        c.set("sy", state.stretch_y as f64);
        // Inputs
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
        // q vars reset to post-init values each frame
        for q in 0..Q_VAR_COUNT {
            c.set(&format!("q{}", q + 1), self.q_after_init[q]);
        }
        // General
        c.set("decay", state.decay as f64);
        c.set("gamma", state.gamma_adj as f64);
        // Wave
        c.set("wave_a", state.wave_alpha as f64);
        c.set("wave_r", state.wave_r as f64);
        c.set("wave_g", state.wave_g as f64);
        c.set("wave_b", state.wave_b as f64);
        c.set("wave_x", state.wave_x as f64);
        c.set("wave_y", state.wave_y as f64);
        c.set("wave_mystery", state.wave_param as f64);
        c.set("wave_mode", state.wave_mode as f64);
        c.set("wave_usedots", state.wave_dots as i32 as f64);
        c.set("wave_thick", state.wave_thick as i32 as f64);
        c.set("wave_additive", state.additive_waves as i32 as f64);
        c.set("wave_brighten", state.maximize_wave_color as i32 as f64);
        // Borders
        c.set("ob_size", state.outer_border_size as f64);
        c.set("ob_r", state.outer_border_r as f64);
        c.set("ob_g", state.outer_border_g as f64);
        c.set("ob_b", state.outer_border_b as f64);
        c.set("ob_a", state.outer_border_a as f64);
        c.set("ib_size", state.inner_border_size as f64);
        c.set("ib_r", state.inner_border_r as f64);
        c.set("ib_g", state.inner_border_g as f64);
        c.set("ib_b", state.inner_border_b as f64);
        c.set("ib_a", state.inner_border_a as f64);
        // Motion vectors
        c.set("mv_x", state.mv_x as f64);
        c.set("mv_y", state.mv_y as f64);
        c.set("mv_dx", state.mv_dx as f64);
        c.set("mv_dy", state.mv_dy as f64);
        c.set("mv_l", state.mv_l as f64);
        c.set("mv_r", state.mv_r as f64);
        c.set("mv_g", state.mv_g as f64);
        c.set("mv_b", state.mv_b as f64);
        c.set("mv_a", state.mv_a as f64);
        // Echo
        c.set("echo_zoom", state.video_echo_zoom as f64);
        c.set("echo_alpha", state.video_echo_alpha as f64);
        c.set("echo_orient", state.video_echo_orientation as f64);
        // Filters
        c.set("darken_center", state.darken_center as i32 as f64);
        c.set("wrap", state.tex_wrap as i32 as f64);
        c.set("invert", state.invert as i32 as f64);
        c.set("brighten", state.brighten as i32 as f64);
        c.set("darken", state.darken as i32 as f64);
        c.set("solarize", state.solarize as i32 as f64);
        // Read-only context
        c.set("meshx", state.frame.mesh_x as f64);
        c.set("meshy", state.frame.mesh_y as f64);
        c.set("pixelsx", state.frame.viewport_width as f64);
        c.set("pixelsy", state.frame.viewport_height as f64);
        c.set("aspectx", state.frame.inv_aspect_x as f64);
        c.set("aspecty", state.frame.inv_aspect_y as f64);
        // Blur
        c.set("blur1_min", state.blur1_min as f64);
        c.set("blur2_min", state.blur2_min as f64);
        c.set("blur3_min", state.blur3_min as f64);
        c.set("blur1_max", state.blur1_max as f64);
        c.set("blur2_max", state.blur2_max as f64);
        c.set("blur3_max", state.blur3_max as f64);
        c.set("blur1_edge_darken", state.blur1_edge_darken as f64);
    }

    fn read_back(&self, state: &mut PresetState) {
        let c = &self.ctx;
        let f = |name: &str| c.get(name) as f32;
        let b = |name: &str| c.get(name) > 0.5;
        let i = |name: &str| c.get(name).round() as i32;

        state.zoom = f("zoom");
        state.zoom_exponent = f("zoomexp");
        state.rot = f("rot");
        state.warp_amount = f("warp");
        state.rot_cx = f("cx");
        state.rot_cy = f("cy");
        state.x_push = f("dx");
        state.y_push = f("dy");
        state.stretch_x = f("sx");
        state.stretch_y = f("sy");
        state.decay = f("decay");
        state.gamma_adj = f("gamma");

        state.wave_alpha = f("wave_a");
        state.wave_r = f("wave_r");
        state.wave_g = f("wave_g");
        state.wave_b = f("wave_b");
        state.wave_x = f("wave_x");
        state.wave_y = f("wave_y");
        state.wave_param = f("wave_mystery");
        state.wave_mode = i("wave_mode");
        state.wave_dots = b("wave_usedots");
        state.wave_thick = b("wave_thick");
        state.additive_waves = b("wave_additive");
        state.maximize_wave_color = b("wave_brighten");

        state.outer_border_size = f("ob_size");
        state.outer_border_r = f("ob_r");
        state.outer_border_g = f("ob_g");
        state.outer_border_b = f("ob_b");
        state.outer_border_a = f("ob_a");
        state.inner_border_size = f("ib_size");
        state.inner_border_r = f("ib_r");
        state.inner_border_g = f("ib_g");
        state.inner_border_b = f("ib_b");
        state.inner_border_a = f("ib_a");

        state.mv_x = f("mv_x");
        state.mv_y = f("mv_y");
        state.mv_dx = f("mv_dx");
        state.mv_dy = f("mv_dy");
        state.mv_l = f("mv_l");
        state.mv_r = f("mv_r");
        state.mv_g = f("mv_g");
        state.mv_b = f("mv_b");
        state.mv_a = f("mv_a");

        state.video_echo_zoom = f("echo_zoom");
        state.video_echo_alpha = f("echo_alpha");
        state.video_echo_orientation = i("echo_orient");

        state.darken_center = b("darken_center");
        state.tex_wrap = b("wrap");
        state.invert = b("invert");
        state.brighten = b("brighten");
        state.darken = b("darken");
        state.solarize = b("solarize");

        state.blur1_min = f("blur1_min");
        state.blur2_min = f("blur2_min");
        state.blur3_min = f("blur3_min");
        state.blur1_max = f("blur1_max");
        state.blur2_max = f("blur2_max");
        state.blur3_max = f("blur3_max");
        state.blur1_edge_darken = f("blur1_edge_darken");

        for q in 0..Q_VAR_COUNT {
            state.frame_q_variables[q] = c.get(&format!("q{}", q + 1));
        }
    }
}

fn compile_opt(code: &str, block: &'static str) -> Result<Option<Program>, PresetError> {
    if code.trim().is_empty() {
        return Ok(None);
    }
    Program::compile(code).map(Some).map_err(|e| PresetError::compile(block, e))
}
