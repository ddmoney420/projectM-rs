//! Port of `MilkdropPreset/PerPixelMesh.{hpp,cpp}` (geometry + per-vertex calc).
//!
//! Builds the warp grid, computes each vertex's `rad`/`ang`, and each frame
//! evaluates the per-pixel equations to fill per-vertex zoom/rot/warp/center/
//! distance/stretch — the attributes the warp vertex shader consumes.

use bytemuck::{Pod, Zeroable};
use pm_preset::Preset;
use pm_render::wgpu;

/// Interleaved per-vertex data uploaded to the GPU each frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct WarpVertex {
    /// Clip-space grid position in `[-1, 1]`.
    pub pos: [f32; 2],
    /// `(radius, angle)`.
    pub rad_ang: [f32; 2],
    /// `(zoom, zoomExp, rot, warp)`.
    pub transforms: [f32; 4],
    /// Warp center `(cx, cy)`.
    pub center: [f32; 2],
    /// Translation `(dx, dy)`.
    pub distance: [f32; 2],
    /// Stretch `(sx, sy)`.
    pub stretch: [f32; 2],
}

impl WarpVertex {
    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        use std::mem::size_of;
        const ATTRS: [wgpu::VertexAttribute; 6] = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 2 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 32, shader_location: 3 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 40, shader_location: 4 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 48, shader_location: 5 },
        ];
        wgpu::VertexBufferLayout {
            array_stride: size_of::<WarpVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// Static per-vertex base data, recomputed only when grid size / aspect change.
#[derive(Clone, Copy, Default)]
struct BaseVertex {
    x: f32,
    y: f32,
    radius: f32,
    angle: f32,
}

pub struct WarpMesh {
    grid_x: usize,
    grid_y: usize,
    aspect_x: f32,
    aspect_y: f32,
    base: Vec<BaseVertex>,
    vertices: Vec<WarpVertex>,
    indices: Vec<u32>,
}

impl WarpMesh {
    /// Build the grid geometry for a `grid_x` by `grid_y` mesh.
    pub fn new(grid_x: usize, grid_y: usize, aspect_x: f32, aspect_y: f32) -> Self {
        let mut mesh = WarpMesh {
            grid_x,
            grid_y,
            aspect_x,
            aspect_y,
            base: Vec::new(),
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        mesh.build_geometry();
        mesh
    }

    pub fn vertices(&self) -> &[WarpVertex] {
        &self.vertices
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn index_count(&self) -> u32 {
        self.indices.len() as u32
    }

    fn build_geometry(&mut self) {
        let (gx, gy) = (self.grid_x, self.grid_y);
        let vertex_count = (gx + 1) * (gy + 1);
        self.base = vec![BaseVertex::default(); vertex_count];
        self.vertices = vec![WarpVertex::zeroed(); vertex_count];

        let mut idx = 0;
        for grid_y in 0..=gy {
            for grid_x in 0..=gx {
                let x = grid_x as f32 / gx as f32 * 2.0 - 1.0;
                let y = grid_y as f32 / gy as f32 * 2.0 - 1.0;
                let radius = (x * self.aspect_x).hypot(y * self.aspect_y);
                let angle = if grid_y == gy / 2 && grid_x == gx / 2 {
                    0.0
                } else {
                    (y * self.aspect_y).atan2(x * self.aspect_x)
                };
                self.base[idx] = BaseVertex { x, y, radius, angle };
                self.vertices[idx].pos = [x, y];
                self.vertices[idx].rad_ang = [radius, angle];
                idx += 1;
            }
        }

        self.build_indices();
    }

    /// Quadrant-ordered triangle list, matching Milkdrop's draw order.
    fn build_indices(&mut self) {
        let (gx, gy) = (self.grid_x, self.grid_y);
        let stride = (gx + 1) as u32;
        let mut indices = Vec::with_capacity(gx * gy * 6);
        for quadrant in 0..4 {
            for slice in 0..gy / 2 {
                for grid_x in 0..gx / 2 {
                    let mut x_ref = grid_x;
                    let mut y_ref = slice;
                    if quadrant & 1 != 0 {
                        x_ref = gx - 1 - x_ref;
                    }
                    if quadrant & 2 != 0 {
                        y_ref = gy - 1 - y_ref;
                    }
                    let v = (x_ref as u32) + (y_ref as u32) * stride;
                    indices.extend_from_slice(&[v, v + 1, v + stride, v + 1, v + stride, v + stride + 1]);
                }
            }
        }
        self.indices = indices;
    }

    /// Evaluate per-vertex motion for this frame. Call after
    /// [`Preset::update_frame`]. Uses the per-pixel code if present, otherwise
    /// applies the uniform per-frame values to every vertex.
    pub fn calculate(&mut self, preset: &mut Preset) -> Result<(), pm_preset::PresetError> {
        let s = preset.state();
        let uniform = [
            s.zoom, s.zoom_exponent, s.rot, s.warp_amount, s.rot_cx, s.rot_cy, s.x_push, s.y_push,
            s.stretch_x, s.stretch_y,
        ];
        let has_pp = preset.has_per_pixel_code();

        for (i, base) in self.base.iter().enumerate() {
            let v = &mut self.vertices[i];
            if has_pp {
                let px = (base.x * 0.5 * self.aspect_x + 0.5) as f64;
                let py = (base.y * 0.5 * self.aspect_y + 0.5) as f64;
                let out = preset.warp_vertex(px, py, base.radius as f64, -(base.angle as f64))?;
                v.transforms = [out.zoom as f32, out.zoom_exponent as f32, out.rot as f32, out.warp as f32];
                v.center = [out.cx as f32, out.cy as f32];
                v.distance = [out.dx as f32, out.dy as f32];
                v.stretch = [out.sx as f32, out.sy as f32];
            } else {
                v.transforms = [uniform[0], uniform[1], uniform[2], uniform[3]];
                v.center = [uniform[4], uniform[5]];
                v.distance = [uniform[6], uniform[7]];
                v.stretch = [uniform[8], uniform[9]];
            }
        }
        Ok(())
    }
}
