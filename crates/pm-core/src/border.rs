//! Port of `MilkdropPreset/Border` — the inner/outer border frames drawn on top
//! of the feedback buffer.
//!
//! Two concentric, axis-aligned square frames in clip space: the outer border
//! occupies the band from the screen edge (radius 1.0) inward by
//! `ob_size`; the inner border the next band inward by `ib_size`. Each is a flat
//! colour with its own alpha, drawn only when that alpha is visible.

use pm_preset::PresetState;

/// One border frame: triangle-list vertices (clip space) and a per-vertex RGBA.
pub struct BorderFrame {
    pub vertices: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
}

/// Build the visible border frames (outer first, then inner on top).
pub fn frames(state: &PresetState) -> Vec<BorderFrame> {
    let mut out = Vec::new();

    // Outer border: band between radius 1.0 and 1.0 - ob_size.
    let ob = state.outer_border_size;
    if state.outer_border_a > 0.001 && ob > 0.0 {
        out.push(ring(
            1.0,
            1.0 - ob,
            [state.outer_border_r, state.outer_border_g, state.outer_border_b, state.outer_border_a],
        ));
    }

    // Inner border: the next band inward, between 1.0 - ob and 1.0 - ob - ib.
    let ib = state.inner_border_size;
    if state.inner_border_a > 0.001 && ib > 0.0 {
        let outer = 1.0 - ob;
        out.push(ring(
            outer,
            outer - ib,
            [state.inner_border_r, state.inner_border_g, state.inner_border_b, state.inner_border_a],
        ));
    }

    out
}

/// A square ring (frame) between an outer and inner half-extent, as a triangle
/// list (4 sides × 2 triangles).
fn ring(outer: f32, inner: f32, color: [f32; 4]) -> BorderFrame {
    let inner = inner.max(0.0);
    // Corners, counter-clockwise: top-left, top-right, bottom-right, bottom-left.
    let o = [[-outer, outer], [outer, outer], [outer, -outer], [-outer, -outer]];
    let i = [[-inner, inner], [inner, inner], [inner, -inner], [-inner, -inner]];

    let mut vertices = Vec::with_capacity(24);
    for side in 0..4 {
        let next = (side + 1) % 4;
        // Quad (o[side], o[next], i[next], i[side]) as two triangles.
        vertices.push(o[side]);
        vertices.push(o[next]);
        vertices.push(i[next]);
        vertices.push(o[side]);
        vertices.push(i[next]);
        vertices.push(i[side]);
    }
    let colors = vec![color; vertices.len()];
    BorderFrame { vertices, colors }
}
