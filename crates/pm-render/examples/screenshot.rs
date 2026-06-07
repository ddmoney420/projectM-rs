//! Renders one frame headlessly and writes it to a PNG you can open.
//!
//! Run with:
//!
//! ```text
//! cargo run -p pm-render --example screenshot
//! ```
//!
//! This exercises the whole `pm-render` path end to end — GPU context, a
//! fullscreen WGSL pass, and CPU readback — and proves it produces a real
//! image, not just green-pixel test asserts. The pattern is a placeholder
//! plasma; real Milkdrop preset shaders arrive in Phase 4/5.

use pm_render::{read_rgba8, FullscreenShader, GpuContext, Texture, TARGET_FORMAT};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 640;

// A colorful radial plasma, in the spirit of a visualizer frame. Uses the
// `uv` from the built-in fullscreen vertex stage.
const FRAGMENT: &str = r#"
@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let p = uv * 2.0 - vec2<f32>(1.0, 1.0); // remap to [-1, 1]
    let r = length(p);
    let a = atan2(p.y, p.x);

    let v = sin(r * 14.0)
          + sin(a * 6.0)
          + sin((p.x + p.y) * 9.0)
          + sin(r * 30.0 - a * 3.0);

    let c = vec3<f32>(
        0.5 + 0.5 * sin(v + 0.0),
        0.5 + 0.5 * sin(v + 2.0944),  // +120 deg
        0.5 + 0.5 * sin(v + 4.1888),  // +240 deg
    );

    // Subtle vignette toward the edges.
    let vignette = 1.0 - 0.4 * r * r;
    return vec4<f32>(c * vignette, 1.0);
}
"#;

fn main() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("No GPU adapter available: {e}");
            std::process::exit(1);
        }
    };
    println!("Rendering on adapter: {}", ctx.adapter_info());

    let target = Texture::new_render_target(&ctx.device, "screenshot", WIDTH, HEIGHT, TARGET_FORMAT);
    let shader = FullscreenShader::new(&ctx.device, TARGET_FORMAT, FRAGMENT);

    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    shader.render(&mut encoder, &target.view, Some(wgpu::Color::BLACK));
    ctx.queue.submit(Some(encoder.finish()));

    let pixels = read_rgba8(&ctx, &target);

    let img = image::RgbaImage::from_raw(WIDTH, HEIGHT, pixels)
        .expect("pixel buffer matches dimensions");
    let path = "pm-render-demo.png";
    img.save(path).expect("failed to write PNG");

    let abs = std::env::current_dir().unwrap().join(path);
    println!("Wrote {WIDTH}x{HEIGHT} image to {}", abs.display());
}
