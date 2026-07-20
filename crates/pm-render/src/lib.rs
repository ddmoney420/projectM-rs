//! `pm-render` — the rendering layer of the projectM port, built on
//! [`wgpu`](https://wgpu.rs).
//!
//! projectM's C++ renderer targets OpenGL directly. This crate provides the
//! same building blocks on wgpu, which compiles our WGSL to **native Metal** on
//! Apple, Vulkan on Linux, and DX12 on Windows from one codebase.
//!
//! # Building blocks
//!
//! - [`GpuContext`] — owns the wgpu device/queue (headless or windowed).
//! - [`Texture`] — a render-target-and-sampling texture.
//! - [`Framebuffer`] — a set of equally-sized color targets (current/previous
//!   ping-pong, multi-stage buffers).
//! - [`RenderContext`] — per-frame global values a preset reads.
//! - [`FullscreenShader`] / [`clear`] — the "run a fragment shader over the
//!   whole target" pattern used by composites, blurs and transitions.
//! - [`read_rgba8`] — copy a target back to CPU for screenshots and tests.
//!
//! # Example (headless): clear a target and read it back
//!
//! ```no_run
//! use pm_render::{GpuContext, Texture, clear, read_rgba8, TARGET_FORMAT};
//!
//! let ctx = GpuContext::headless().expect("no GPU adapter");
//! let target = Texture::new_render_target(&ctx.device, "main", 64, 64, TARGET_FORMAT);
//!
//! let mut enc = ctx.device.create_command_encoder(&Default::default());
//! clear(&mut enc, &target.view, wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
//! ctx.queue.submit(Some(enc.finish()));
//!
//! let pixels = read_rgba8(&ctx, &target);
//! assert_eq!(&pixels[0..4], &[255, 0, 0, 255]);
//! ```

mod blit;
mod context;
mod framebuffer;
mod mesh;
mod pass;
mod readback;
mod render_context;
mod texture;

pub use blit::Blit;
pub use context::{GpuContext, GpuError};
pub use framebuffer::Framebuffer;
pub use mesh::{Color, ColoredPoint, Point};
pub use pass::{clear, FullscreenShader, FULLSCREEN_VERTEX_WGSL};
pub use readback::read_rgba8;
pub use render_context::RenderContext;
pub use texture::{live_texture_bytes, live_texture_count, Texture, TARGET_FORMAT};

// Re-export wgpu so downstream crates use a matching version.
pub use wgpu;
