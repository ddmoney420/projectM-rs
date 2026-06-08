//! Render a single `.milk` preset file to a PNG, reporting whether its custom
//! composite shader was used.
//!
//! ```text
//! cargo run -p pm-core --example render_preset -- <file.milk>
//! ```

use pm_audio::{FrameAudioData, WAVEFORM_SAMPLES};
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 600;
const FRAMES: i32 = 150;

fn frame_audio(frame: i32) -> FrameAudioData {
    let t = frame as f32 / 30.0;
    let mut audio = FrameAudioData::default();
    for i in 0..WAVEFORM_SAMPLES {
        let p = i as f32 / WAVEFORM_SAMPLES as f32;
        let s = (p * 24.0 + t * 2.0).sin() * 16.0 + (p * 7.0 - t).sin() * 9.0;
        audio.waveform_left[i] = s;
        audio.waveform_right[i] = s * 0.8;
    }
    audio.bass = 1.0 + 0.8 * (t * 1.5).sin().abs();
    audio.mid = 1.0 + 0.5 * (t * 2.3).sin().abs();
    audio.treb = 1.0 + 0.4 * (t * 3.1).sin().abs();
    audio.vol = (audio.bass + audio.mid + audio.treb) / 3.0;
    audio
}

fn collect(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
            out.push(p);
        }
    }
}

fn renders_content(ctx: &GpuContext, preset: Preset) -> bool {
    let mut engine = WarpEngine::new(ctx, preset, 160, 160);
    for frame in 0..8 {
        let _ = engine.render_frame(ctx, frame as f32 / 60.0, frame, frame_audio(frame));
    }
    let px = read_rgba8(ctx, engine.display_texture());
    px.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30).count() > 100
}

fn main() {
    let arg = std::env::args().nth(1).expect("usage: render_preset <file.milk | dir> [count]");
    let count_mode = std::env::args().nth(2).as_deref() == Some("count");
    let ctx = GpuContext::headless().expect("no GPU adapter");

    // Resolve to a list of candidate presets.
    let path = std::path::Path::new(&arg);
    let mut candidates = Vec::new();
    if path.is_dir() {
        collect(path, &mut candidates);
    } else {
        candidates.push(path.to_path_buf());
    }

    if count_mode {
        let sample = candidates.len().min(400);
        let mut rendered = 0;
        let mut loaded = 0;
        for p in candidates.iter().take(sample) {
            let Ok(bytes) = std::fs::read(p) else { continue };
            let Ok(preset) = Preset::load(&String::from_utf8_lossy(&bytes)) else { continue };
            loaded += 1;
            if renders_content(&ctx, preset) {
                rendered += 1;
            }
        }
        println!(
            "Of {loaded} loaded presets (first {sample}), {rendered} render visible content ({:.1}%)",
            100.0 * rendered as f64 / loaded.max(1) as f64
        );
        return;
    }

    // Find the first preset that actually uses a custom composite shader.
    let mut chosen = None;
    for (i, p) in candidates.iter().enumerate() {
        if i >= 800 {
            break;
        }
        let Ok(bytes) = std::fs::read(p) else { continue };
        let content = String::from_utf8_lossy(&bytes);
        let Ok(preset) = Preset::load(&content) else { continue };
        let engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);
        if engine.uses_custom_composite() {
            println!("custom composite: {}", p.display());
            chosen = Some(engine);
            break;
        }
    }

    let Some(mut engine) = chosen else {
        println!("no preset with a translatable custom composite found in the first 800");
        return;
    };

    for frame in 0..FRAMES {
        engine.render_frame(&ctx, frame as f32 / 30.0, frame, frame_audio(frame)).expect("render");
    }

    let pixels = read_rgba8(&ctx, engine.display_texture());
    let img = image::RgbaImage::from_raw(SIZE, SIZE, pixels).unwrap();
    img.save("pm-custom-composite.png").unwrap();

    // Also dump the feedback buffer to distinguish "composite is wrong" from
    // "feedback is empty".
    let fb = read_rgba8(&ctx, engine.main_texture());
    let nonblack = fb.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 24).count();
    image::RgbaImage::from_raw(SIZE, SIZE, fb).unwrap().save("pm-feedback.png").unwrap();
    println!("wrote pm-custom-composite.png + pm-feedback.png (feedback lit pixels: {nonblack})");
}
