//! Port of `MilkdropPreset/CustomShape.{hpp,cpp}` and `ShapePerFrameContext`.
//!
//! Each enabled `shape_N` draws `num_inst` instances of a filled N-gon (center
//! color → edge color gradient) plus an optional border, with per-instance
//! per-frame code positioning/coloring each one (reading the `instance` var).

use crate::error::PresetError;
use crate::state::{CustomShapeConfig, PresetState, T_VAR_COUNT};
use pm_eval::{Context, Program};

const PI: f64 = std::f64::consts::PI;

/// Geometry for one shape instance.
pub struct CustomShapeOutput {
    /// Fill triangles (triangle list): `sides * 3` vertices.
    pub fill_vertices: Vec<[f32; 2]>,
    pub fill_colors: Vec<[f32; 4]>,
    /// Border line loop (closed; empty if no border).
    pub border_points: Vec<[f32; 2]>,
    pub border_color: [f32; 4],
    pub additive: bool,
}

pub struct CustomShape {
    index: usize,
    per_frame: Context,
    init_prog: Option<Program>,
    frame_prog: Option<Program>,
    t_after_init: [f64; T_VAR_COUNT],
}

impl CustomShape {
    pub fn new(state: &PresetState, index: usize) -> Result<Option<Self>, PresetError> {
        if !state.custom_shapes[index].enabled {
            return Ok(None);
        }
        let mut per_frame = Context::new();
        let init_prog = compile_opt(&mut per_frame, &state.custom_shape_init_code[index], "shape_init")?;
        let frame_prog =
            compile_opt(&mut per_frame, &state.custom_shape_per_frame_code[index], "shape_per_frame")?;

        let mut shape = CustomShape {
            index,
            per_frame,
            init_prog,
            frame_prog,
            t_after_init: [0.0; T_VAR_COUNT],
        };
        shape.evaluate_init(state)?;
        Ok(Some(shape))
    }

    fn evaluate_init(&mut self, state: &PresetState) -> Result<(), PresetError> {
        let cfg = state.custom_shapes[self.index].clone();
        self.load_state(state, &cfg, 0, &[0.0; T_VAR_COUNT]);
        if let Some(p) = &self.init_prog {
            p.run(&mut self.per_frame).map_err(|e| PresetError::eval("shape_init", e))?;
        }
        for t in 0..T_VAR_COUNT {
            self.t_after_init[t] = self.per_frame.get(&format!("t{}", t + 1));
        }
        Ok(())
    }

    /// Load all per-frame inputs for one instance (config + q/t + instance).
    fn load_state(&mut self, state: &PresetState, cfg: &CustomShapeConfig, instance: i32, t_vars: &[f64; T_VAR_COUNT]) {
        let c = &mut self.per_frame;
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
        for (q, &v) in state.frame_q_variables.iter().enumerate() {
            c.set(&format!("q{}", q + 1), v);
        }
        for (t, &v) in t_vars.iter().enumerate() {
            c.set(&format!("t{}", t + 1), v);
        }
        c.set("x", cfg.x as f64);
        c.set("y", cfg.y as f64);
        c.set("rad", cfg.radius as f64);
        c.set("ang", cfg.angle as f64);
        c.set("tex_ang", cfg.tex_ang as f64);
        c.set("tex_zoom", cfg.tex_zoom as f64);
        c.set("sides", cfg.sides as f64);
        c.set("additive", cfg.additive as i32 as f64);
        c.set("textured", cfg.textured as i32 as f64);
        c.set("num_inst", cfg.instances as f64);
        c.set("instance", instance as f64);
        c.set("thick", cfg.thick_outline as i32 as f64);
        c.set("r", cfg.r as f64);
        c.set("g", cfg.g as f64);
        c.set("b", cfg.b as f64);
        c.set("a", cfg.a as f64);
        c.set("r2", cfg.r2 as f64);
        c.set("g2", cfg.g2 as f64);
        c.set("b2", cfg.b2 as f64);
        c.set("a2", cfg.a2 as f64);
        c.set("border_r", cfg.border_r as f64);
        c.set("border_g", cfg.border_g as f64);
        c.set("border_b", cfg.border_b as f64);
        c.set("border_a", cfg.border_a as f64);
    }

    /// Run the per-frame code per instance and build each one's geometry.
    pub fn generate(&mut self, state: &PresetState) -> Result<Vec<CustomShapeOutput>, PresetError> {
        let cfg = state.custom_shapes[self.index].clone();
        let aspect_y = state.frame.aspect_y as f64;
        let t_init = self.t_after_init;
        let mut out = Vec::new();

        for instance in 0..cfg.instances.max(0) {
            self.load_state(state, &cfg, instance, &t_init);
            if let Some(p) = &self.frame_prog {
                p.run(&mut self.per_frame).map_err(|e| PresetError::eval("shape_per_frame", e))?;
            }
            let c = &self.per_frame;

            let sides = (c.get("sides").round() as i32).clamp(3, 100);
            let x = c.get("x");
            let y = c.get("y");
            let rad = c.get("rad");
            let ang = c.get("ang");
            let additive = c.get("additive") > 0.5;

            let center = [(x * 2.0 - 1.0) as f32, (y * 2.0 - 1.0) as f32];
            let center_color = modulo4(c.get("r"), c.get("g"), c.get("b"), c.get("a"));
            let edge_color = modulo4(c.get("r2"), c.get("g2"), c.get("b2"), c.get("a2"));

            // Perimeter vertices.
            let perim: Vec<[f32; 2]> = (0..sides)
                .map(|i| {
                    let angle = i as f64 / sides as f64 * PI * 2.0 + ang + PI * 0.25;
                    [
                        center[0] + (rad * angle.cos() * aspect_y) as f32,
                        center[1] + (rad * angle.sin()) as f32,
                    ]
                })
                .collect();

            // Triangle-list fill (fan around the center).
            let mut fill_vertices = Vec::with_capacity(sides as usize * 3);
            let mut fill_colors = Vec::with_capacity(sides as usize * 3);
            for i in 0..sides as usize {
                let j = (i + 1) % sides as usize;
                fill_vertices.push(center);
                fill_colors.push(center_color);
                fill_vertices.push(perim[i]);
                fill_colors.push(edge_color);
                fill_vertices.push(perim[j]);
                fill_colors.push(edge_color);
            }

            // Border loop.
            let border_a = c.get("border_a");
            let (border_points, border_color) = if border_a > 0.0001 {
                let mut bp = perim.clone();
                bp.push(perim[0]);
                (
                    bp,
                    [
                        c.get("border_r") as f32,
                        c.get("border_g") as f32,
                        c.get("border_b") as f32,
                        border_a as f32,
                    ],
                )
            } else {
                (Vec::new(), [0.0; 4])
            };

            out.push(CustomShapeOutput {
                fill_vertices,
                fill_colors,
                border_points,
                border_color,
                additive,
            });
        }

        Ok(out)
    }
}

fn modulo4(r: f64, g: f64, b: f64, a: f64) -> [f32; 4] {
    [modulo(r), modulo(g), modulo(b), modulo(a)]
}

fn modulo(x: f64) -> f32 {
    let m = 256.0f32 / 255.0;
    let x = x as f32;
    ((x % m) + m) % m
}

fn compile_opt(ctx: &mut Context, code: &str, block: &'static str) -> Result<Option<Program>, PresetError> {
    if code.trim().is_empty() {
        return Ok(None);
    }
    Program::compile(ctx, code).map(Some).map_err(|e| PresetError::compile(block, e))
}
