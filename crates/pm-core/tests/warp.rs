//! Headless test: the warp pass actually transforms the feedback buffer.
//! Skips (rather than fails) when no GPU adapter is available.

use pm_audio::FrameAudioData;
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 64;

/// Seed image: white top half, black bottom half (a sharp horizontal edge).
fn split_image() -> Vec<u8> {
    let mut data = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * SIZE + x) * 4) as usize;
            let v = if y < SIZE / 2 { 255 } else { 0 };
            data[i] = v;
            data[i + 1] = v;
            data[i + 2] = v;
            data[i + 3] = 255;
        }
    }
    data
}

#[test]
fn warp_zoom_transforms_feedback_buffer() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping warp test");
        return;
    };

    // Strong zoom, no decay/warp/rotation: the edge should march toward center.
    let preset = Preset::load("zoom=1.1\nfDecay=1.0\nwarp=0\nrot=0\nbTexWrap=0").unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);

    let seed = split_image();
    engine.seed(&ctx, &seed);

    for frame in 0..20 {
        engine.render_frame(&ctx, frame as f32 / 60.0, frame, FrameAudioData::default()).unwrap();
    }

    let out = read_rgba8(&ctx, engine.main_texture());
    assert_eq!(out.len(), seed.len());

    // Content is preserved (not faded to black): both bright and dark present.
    let bright = out.chunks_exact(4).filter(|p| p[0] > 200).count();
    let dark = out.chunks_exact(4).filter(|p| p[0] < 55).count();
    assert!(bright > 0, "expected bright pixels to survive (decay=1)");
    assert!(dark > 0, "expected dark pixels");

    // The buffer changed: zoom pushed the white region downward from the top
    // half, so the white-pixel count differs from the seed's exact half.
    let seed_bright = seed.chunks_exact(4).filter(|p| p[0] > 200).count();
    assert_ne!(bright, seed_bright, "warp should have moved the edge");
}

#[test]
fn warp_runs_with_per_pixel_code() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // Per-pixel rotation that varies with radius — exercises the per-vertex path.
    let milk = "\
fDecay=0.97
per_frame_1=`zoom = 1.01;
per_pixel_1=`rot = rot + 0.3 * (rad - 0.5); zoom = zoom + 0.02 * sin(ang);
";
    let preset = Preset::load(milk).unwrap();
    assert!(preset.has_per_pixel_code());
    let mut engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);
    engine.seed(&ctx, &split_image());

    // Should run many frames without panicking and produce a valid image.
    for frame in 0..10 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, FrameAudioData::default()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.main_texture());
    assert_eq!(out.len(), (SIZE * SIZE * 4) as usize);
}
