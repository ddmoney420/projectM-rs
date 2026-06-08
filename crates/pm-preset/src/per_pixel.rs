//! Port of `MilkdropPreset/PerPixelContext.{hpp,cpp}`.
//!
//! Runs the preset's `per_pixel` equations once per warp-mesh vertex. Each
//! vertex loads the per-frame motion results plus the vertex position
//! (`x`, `y`, `rad`, `ang`), executes the code, and reads back the per-vertex
//! motion values that warp the mesh.

use crate::error::PresetError;
use crate::state::{PresetState, Q_VAR_COUNT};
use pm_eval::{Context, Program, VarSlot};

/// Cached slot handles for the variables set/read every vertex, so the hot loop
/// avoids name lookups entirely.
struct Slots {
    zoom: VarSlot,
    zoomexp: VarSlot,
    rot: VarSlot,
    warp: VarSlot,
    cx: VarSlot,
    cy: VarSlot,
    dx: VarSlot,
    dy: VarSlot,
    sx: VarSlot,
    sy: VarSlot,
    x: VarSlot,
    y: VarSlot,
    rad: VarSlot,
    ang: VarSlot,
}

impl Slots {
    fn resolve(ctx: &mut Context) -> Self {
        Slots {
            zoom: ctx.variable_slot("zoom"),
            zoomexp: ctx.variable_slot("zoomexp"),
            rot: ctx.variable_slot("rot"),
            warp: ctx.variable_slot("warp"),
            cx: ctx.variable_slot("cx"),
            cy: ctx.variable_slot("cy"),
            dx: ctx.variable_slot("dx"),
            dy: ctx.variable_slot("dy"),
            sx: ctx.variable_slot("sx"),
            sy: ctx.variable_slot("sy"),
            x: ctx.variable_slot("x"),
            y: ctx.variable_slot("y"),
            rad: ctx.variable_slot("rad"),
            ang: ctx.variable_slot("ang"),
        }
    }
}

/// Per-vertex motion outputs that drive the mesh warp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerPixelOutput {
    pub zoom: f64,
    pub zoom_exponent: f64,
    pub rot: f64,
    pub warp: f64,
    pub cx: f64,
    pub cy: f64,
    pub dx: f64,
    pub dy: f64,
    pub sx: f64,
    pub sy: f64,
}

pub struct PerPixelContext {
    ctx: Context,
    program: Option<Program>,
    slots: Slots,
}

impl PerPixelContext {
    pub fn new(state: &PresetState) -> Result<Self, PresetError> {
        let mut ctx = Context::new();
        let program = if state.per_pixel_code.trim().is_empty() {
            None
        } else {
            Some(Program::compile(&mut ctx, &state.per_pixel_code).map_err(|e| PresetError::compile("per_pixel", e))?)
        };
        let slots = Slots::resolve(&mut ctx);
        Ok(PerPixelContext { ctx, program, slots })
    }

    /// True if the preset has any per-pixel code (otherwise the mesh is uniform).
    pub fn has_code(&self) -> bool {
        self.program.is_some()
    }

    /// Set frame-constant inputs once before iterating the mesh vertices: audio,
    /// time, viewport/mesh/aspect, and the per-frame q1..q32 values.
    pub fn begin_frame(&mut self, state: &PresetState, q_vars: &[f64; Q_VAR_COUNT]) {
        let c = &mut self.ctx;
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
        c.set("meshx", state.frame.mesh_x as f64);
        c.set("meshy", state.frame.mesh_y as f64);
        c.set("pixelsx", state.frame.viewport_width as f64);
        c.set("pixelsy", state.frame.viewport_height as f64);
        c.set("aspectx", state.frame.inv_aspect_x as f64);
        c.set("aspecty", state.frame.inv_aspect_y as f64);
        for (q, &value) in q_vars.iter().enumerate() {
            c.set(&format!("q{}", q + 1), value);
        }
    }

    /// Evaluate the per-pixel code for a single vertex. `x`/`y` are mesh UVs and
    /// `rad`/`ang` are the polar coordinates the mesh computes for the vertex.
    /// Returns the warped motion values for that vertex.
    pub fn execute_vertex(
        &mut self,
        state: &PresetState,
        x: f64,
        y: f64,
        rad: f64,
        ang: f64,
    ) -> Result<PerPixelOutput, PresetError> {
        let s = &self.slots;
        let c = &mut self.ctx;
        // Motion values reload from the per-frame results for every vertex.
        c.set_slot(s.zoom, state.zoom as f64);
        c.set_slot(s.zoomexp, state.zoom_exponent as f64);
        c.set_slot(s.rot, state.rot as f64);
        c.set_slot(s.warp, state.warp_amount as f64);
        c.set_slot(s.cx, state.rot_cx as f64);
        c.set_slot(s.cy, state.rot_cy as f64);
        c.set_slot(s.dx, state.x_push as f64);
        c.set_slot(s.dy, state.y_push as f64);
        c.set_slot(s.sx, state.stretch_x as f64);
        c.set_slot(s.sy, state.stretch_y as f64);
        // Per-vertex position.
        c.set_slot(s.x, x);
        c.set_slot(s.y, y);
        c.set_slot(s.rad, rad);
        c.set_slot(s.ang, ang);

        if let Some(prog) = &self.program {
            prog.run(c).map_err(|e| PresetError::eval("per_pixel", e))?;
        }

        Ok(PerPixelOutput {
            zoom: c.get_slot(s.zoom),
            zoom_exponent: c.get_slot(s.zoomexp),
            rot: c.get_slot(s.rot),
            warp: c.get_slot(s.warp),
            cx: c.get_slot(s.cx),
            cy: c.get_slot(s.cy),
            dx: c.get_slot(s.dx),
            dy: c.get_slot(s.dy),
            sx: c.get_slot(s.sx),
            sy: c.get_slot(s.sy),
        })
    }
}
