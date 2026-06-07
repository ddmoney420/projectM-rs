//! Renders a Milkdrop warp/feedback "tunnel" headlessly and writes a PNG.
//!
//! ```text
//! cargo run -p pm-core --example warp_demo
//! ```
//!
//! Seeds a colorful pattern, then runs the preset's zoom/rotate/warp feedback
//! for many frames — the classic Milkdrop flow — and saves the result.

use pm_audio::FrameAudioData;
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 600;
const FRAMES: i32 = 120;

/// A vivid seed: colored concentric rings plus a bright diagonal, so the
/// zoom/rotation feedback is clearly visible.
fn seed_image() -> Vec<u8> {
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    let cx = SIZE as f32 / 2.0;
    let cy = SIZE as f32 / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 - cx) / cx;
            let dy = (y as f32 - cy) / cy;
            let r = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx);

            let ring = (r * 18.0).sin() * 0.5 + 0.5;
            let spokes = (a * 6.0).sin() * 0.5 + 0.5;
            let red = (ring * 255.0) as u8;
            let green = (spokes * 255.0) as u8;
            let blue = ((1.0 - r).clamp(0.0, 1.0) * 255.0) as u8;

            let i = ((y * SIZE + x) * 4) as usize;
            data[i] = red;
            data[i + 1] = green;
            data[i + 2] = blue;
            data[i + 3] = 255;
        }
    }
    data
}

fn main() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("No GPU adapter available: {e}");
            std::process::exit(1);
        }
    };
    println!("Rendering on adapter: {}", ctx.adapter_info());

    // A gently zooming, rotating, warping preset with mild decay -> a tunnel.
    let milk = "\
fDecay=0.992
zoom=1.018
rot=0.010
warp=1.0
fWarpScale=1.2
bTexWrap=1
per_pixel_1=`rot = rot + 0.20 * (rad - 0.5);
per_pixel_2=`zoom = zoom + 0.020 * sin(ang * 2.0);
";
    let preset = Preset::load(milk).expect("preset loads");
    let mut engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);
    engine.seed(&ctx, &seed_image());

    for frame in 0..FRAMES {
        engine
            .render_frame(&ctx, frame as f32 / 30.0, frame, FrameAudioData::default())
            .expect("frame renders");
    }

    let pixels = read_rgba8(&ctx, engine.main_texture());
    let img = image::RgbaImage::from_raw(SIZE, SIZE, pixels).expect("dimensions match");
    let path = "pm-warp-demo.png";
    img.save(path).expect("write PNG");

    let abs = std::env::current_dir().unwrap().join(path);
    println!("Rendered {FRAMES} warp frames to {}", abs.display());
}
