//! Headless test: the warp pass actually transforms the feedback buffer.
//! Skips (rather than fails) when no GPU adapter is available.

use pm_audio::{FrameAudioData, WAVEFORM_SAMPLES};
use pm_core::{generate_waveform, WarpEngine};
use pm_preset::{FrameParams, Preset};
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 64;

/// A loud sawtooth-ish waveform for one frame.
fn loud_audio() -> FrameAudioData {
    let mut a = FrameAudioData::default();
    for i in 0..WAVEFORM_SAMPLES {
        let p = i as f32 / WAVEFORM_SAMPLES as f32;
        a.waveform_left[i] = (p * 20.0).sin() * 40.0;
        a.waveform_right[i] = (p * 20.0).cos() * 40.0;
    }
    a.bass = 2.0;
    a.mid = 1.5;
    a.treb = 1.2;
    a.vol = 1.5;
    a
}

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

#[test]
fn waveform_circle_generates_a_loop() {
    // Mode 0 (Circle) -> a closed loop with points (CPU only, no GPU).
    let mut preset = Preset::load("nWaveMode=0\nfWaveScale=1.0").unwrap();
    let frame = FrameParams { viewport_width: 512, viewport_height: 512, ..FrameParams::default() };
    preset.update_frame(frame, loud_audio()).unwrap();

    let geo = generate_waveform(preset.state());
    assert!(geo.is_loop, "circle mode is a loop");
    assert!(geo.points.len() > 100, "expected many points, got {}", geo.points.len());
    // All points finite.
    assert!(geo.points.iter().all(|p| p[0].is_finite() && p[1].is_finite()));
}

#[test]
fn waveform_line_is_not_a_loop() {
    let mut preset = Preset::load("nWaveMode=6\nfWaveScale=1.0").unwrap();
    let frame = FrameParams { viewport_width: 512, viewport_height: 512, ..FrameParams::default() };
    preset.update_frame(frame, loud_audio()).unwrap();
    let geo = generate_waveform(preset.state());
    assert!(!geo.is_loop, "line mode is not a loop");
    assert!(!geo.points.is_empty());
}

#[test]
fn waveform_all_standard_modes_generate_finite_geometry() {
    // Every standard mode 0..=8 must produce finite, non-empty geometry, with
    // the right loop flag and a second line only for the double-line mode.
    for mode in 0..=8 {
        let mut preset = Preset::load(&format!("nWaveMode={mode}\nfWaveScale=1.0")).unwrap();
        let frame = FrameParams { viewport_width: 512, viewport_height: 512, ..FrameParams::default() };
        preset.update_frame(frame, loud_audio()).unwrap();
        let geo = generate_waveform(preset.state());

        assert!(!geo.points.is_empty(), "mode {mode}: empty");
        assert!(
            geo.points.iter().all(|p| p[0].is_finite() && p[1].is_finite()),
            "mode {mode}: non-finite vertex"
        );
        assert_eq!(geo.is_loop, matches!(mode, 0 | 1), "mode {mode}: loop flag");
        assert_eq!(geo.points2.is_some(), mode == 7, "mode {mode}: second line only for double-line");
        if let Some(p2) = &geo.points2 {
            assert!(p2.iter().all(|p| p[0].is_finite() && p[1].is_finite()), "mode {mode}: w2 non-finite");
        }
    }
}

#[test]
fn full_pipeline_injects_bright_content() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // Bright additive circular waveform; the composite output should light up.
    let milk = "\
fDecay=0.9
nWaveMode=0
bAdditiveWaves=1
bMaximizeWaveColor=1
fWaveAlpha=1.0
fWaveScale=2.0
wave_r=1.0
wave_g=1.0
wave_b=1.0
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    // Start from black; the waveform must add visible content on its own.
    engine.seed(&ctx, &vec![0u8; 128 * 128 * 4]);

    for frame in 0..15 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }

    let out = read_rgba8(&ctx, engine.display_texture());
    let lit = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 60).count();
    assert!(lit > 50, "waveform should inject visible content, only {lit} lit pixels");
}

#[test]
fn custom_warp_shader_compiles_and_renders() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let milk = "\
MILKDROP_PRESET_VERSION=201
fDecay=0.96
nWaveMode=0
bAdditiveWaves=1
fWaveAlpha=1.0
fWaveScale=2.0
warp_1=`shader_body
warp_2=`{
warp_3=`ret = tex2D(sampler_main, uv).xyz;
warp_4=`ret *= 0.98;
warp_5=`ret.g = ret.g * (1.0 + 0.2*treb);
warp_6=`}
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    assert!(engine.uses_custom_warp(), "custom warp shader should compile to valid WGSL");

    for frame in 0..12 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.display_texture());
    let lit = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
    assert!(lit > 50, "custom warp + waveform should produce visible content");
}

#[test]
fn custom_warp_with_3d_noise_renders() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // A warp shader that samples the 3D noise-volume texture (texture_3d binding
    // path) plus the 2D noise texture — must build a valid pipeline and render
    // without a bind-group dimension mismatch.
    let milk = "\
MILKDROP_PRESET_VERSION=201
nWaveMode=0
bAdditiveWaves=1
fWaveScale=2.0
warp_1=`shader_body
warp_2=`{
warp_3=`float3 n3 = tex3D(sampler_noisevol_hq, float3(uv, time*0.1)).xyz;
warp_4=`float3 n2 = tex2D(sampler_noise_lq, uv*2.0).xyz;
warp_5=`ret = tex2D(sampler_main, uv).xyz * 0.97 + 0.05*n3 + 0.03*n2;
warp_6=`}
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    assert!(engine.uses_custom_warp(), "3D-noise warp shader should compile to valid WGSL");
    for frame in 0..8 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.display_texture());
    let lit = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
    assert!(lit > 50, "noise-sampling warp should produce visible content");
}

#[test]
fn warp_shader_sampling_blur_renders() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // GetBlur1/2/3 expand to tex2D(sampler_blur1/2/3, ...); the blur chain must
    // bind real blur textures so the pipeline builds and renders.
    let milk = "\
MILKDROP_PRESET_VERSION=201
nWaveMode=0
bAdditiveWaves=1
fWaveScale=2.0
warp_1=`shader_body
warp_2=`{
warp_3=`float3 b1 = GetBlur1(uv);
warp_4=`float3 b3 = GetBlur3(uv);
warp_5=`ret = tex2D(sampler_main, uv).xyz * 0.95 + 0.05*b1 + 0.05*b3;
warp_6=`}
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    assert!(engine.uses_custom_warp(), "blur-sampling warp shader should compile to valid WGSL");
    for frame in 0..8 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.display_texture());
    let lit = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count();
    assert!(lit > 50, "blur-sampling warp should produce visible content");
}

#[test]
fn default_composite_with_echo_and_filter_renders() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // No composite shader -> the classic VideoEcho + Filters path. Strong echo,
    // gamma, and the invert filter must produce a valid, non-empty frame.
    let milk = "\
nWaveMode=0
fWaveScale=1.5
bAdditiveWaves=1
fVideoEchoZoom=1.25
fVideoEchoAlpha=0.5
nVideoEchoOrientation=3
fGammaAdj=2.0
bInvert=1
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    assert!(!engine.uses_custom_composite(), "should use the built-in composite");
    for frame in 0..6 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.display_texture());
    // Invert turns the black background white, so most pixels are bright.
    let bright = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 600).count();
    assert!(bright > out.len() / 4 / 2, "invert filter should brighten the background");
}

#[test]
fn border_frames_gated_on_visible_alpha() {
    use pm_core::border_frames;
    // No alpha -> no frames.
    let p = Preset::load("ob_size=0.05\nib_size=0.05").unwrap();
    assert!(border_frames(p.state()).is_empty(), "invisible borders produce no geometry");

    // Both visible -> two frames, each a non-empty triangle list with the color.
    let milk = "ob_size=0.06\nob_a=1.0\nob_r=1.0\nib_size=0.03\nib_a=1.0\nib_b=1.0";
    let p = Preset::load(milk).unwrap();
    let frames = border_frames(p.state());
    assert_eq!(frames.len(), 2, "outer + inner");
    assert_eq!(frames[0].vertices.len() % 3, 0, "triangle list");
    assert!(!frames[0].vertices.is_empty());
    assert_eq!(frames[0].colors[0], [1.0, 0.0, 0.0, 1.0], "outer border color");
    // All vertices within the clip square.
    assert!(frames[0].vertices.iter().all(|v| v[0].abs() <= 1.0 && v[1].abs() <= 1.0));
}

#[test]
fn motion_vectors_render_into_feedback() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // Warp flow + a visible motion-vector grid: the overlay must add content and
    // not panic (it samples the warp motion-field texture in its vertex shader).
    let milk = "\
zoom=1.05
rot=0.08
warp=0.2
fDecay=0.95
mv_x=16
mv_y=12
mv_l=2.0
mv_r=1.0
mv_g=1.0
mv_b=1.0
mv_a=1.0
fVideoEchoAlpha=0.0
";
    let preset = Preset::load(milk).unwrap();
    let mut engine = WarpEngine::new(&ctx, preset, 128, 128);
    engine.seed(&ctx, &vec![0u8; 128 * 128 * 4]);
    for frame in 0..8 {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, loud_audio()).unwrap();
    }
    let out = read_rgba8(&ctx, engine.display_texture());
    let lit = out.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 60).count();
    assert!(lit > 50, "motion vectors should inject visible content, only {lit} lit");
}
