//! Bucket E visual verification harness (temporary).
//!
//! Renders a fixed list of matrix/`mul`-using presets to PNGs under
//! `bucket-e-visuals/<label>-<TAG>.png`, with deterministic audio + a fixed
//! frame sequence so the ONLY difference between two runs is the shader
//! codegen. Run once on the new code (TAG=after) and once after git-stashing
//! the codegen change (TAG=before), then eyeball the pairs.
//!
//! ```text
//! cargo run -p pm-core --example matrix_visual --release -- <TAG>
//! ```

use pm_audio::{FrameAudioData, WAVEFORM_SAMPLES};
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 600;
const FRAMES: i32 = 150;

// (label, path). Mix of previously-VALID `mul` shaders (rendering changes with
// the operand flip) and previously-REJECTED Bucket E shaders (now validate).
const PRESETS: &[(&str, &str)] = &[
    // previously VALID + live matrix `mul` -> rendering CHANGES (the silent-
    // transpose correction; not detectable by valid-set diff, must be eyeballed).
    ("01_deep_blue_VALID_changes", "classic/tryptonaut/martin - deep blue.milk"),
    ("02_liquid_gold_VALID_changes", "classic/tryptonaut/martin - liquid gold.milk"),
    ("03_the_beast_VALID_changes", "classic/tryptonaut/martin - the beast.milk"),
    ("04_jellyfish_dance_VALID_changes", "classic/mischa_collection/martin - jellyfish dance.milk"),
    // previously REJECTED (Bucket E), now valid
    ("05_fireworks_NEWLY_VALID", "cream-of-the-crop/Dancer/Lasers/2009 4th of July with AdamFX n Martin - into the fireworks ft Armandio C A.milk"),
];

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

fn main() {
    let tag = std::env::args().nth(1).unwrap_or_else(|| "after".into());
    let ctx = GpuContext::headless().expect("no GPU adapter");

    // Batch mode: `matrix_visual <tag> <list-file>` renders each listed .milk to
    // bucket-e-batch/NNN-<tag>.png at small size (for cmp-based diff scanning).
    if let Some(list) = std::env::args().nth(2) {
        let out = std::path::Path::new("bucket-e-batch");
        std::fs::create_dir_all(out).unwrap();
        let body = std::fs::read_to_string(&list).unwrap();
        for (i, line) in body.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let Ok(bytes) = std::fs::read(line.trim()) else { continue };
            let Ok(preset) = Preset::load(&String::from_utf8_lossy(&bytes)) else { continue };
            let mut engine = WarpEngine::new(&ctx, preset, 256, 256);
            for frame in 0..90 {
                let _ = engine.render_frame(&ctx, frame as f32 / 30.0, frame, frame_audio(frame));
            }
            let px = read_rgba8(&ctx, engine.display_texture());
            image::RgbaImage::from_raw(256, 256, px).unwrap()
                .save(out.join(format!("{i:03}-{tag}.png"))).unwrap();
        }
        println!("batch {tag} done");
        return;
    }

    let root = std::path::Path::new("C:/My-Workspace/projectm-presets");
    let out = std::path::Path::new("bucket-e-visuals");
    std::fs::create_dir_all(out).unwrap();

    for (label, rel) in PRESETS {
        let path = root.join(rel);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("MISSING: {}", path.display());
            continue;
        };
        let content = String::from_utf8_lossy(&bytes).into_owned();
        let Ok(preset) = Preset::load(&content) else {
            println!("LOAD-FAIL: {label}");
            continue;
        };
        let mut engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);
        let (cc, cw) = (engine.uses_custom_composite(), engine.uses_custom_warp());
        for frame in 0..FRAMES {
            engine.render_frame(&ctx, frame as f32 / 30.0, frame, frame_audio(frame)).expect("render");
        }
        let pixels = read_rgba8(&ctx, engine.display_texture());
        let lit = pixels.chunks_exact(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 24).count();
        let file = out.join(format!("{label}-{tag}.png"));
        image::RgbaImage::from_raw(SIZE, SIZE, pixels).unwrap().save(&file).unwrap();
        println!("{label}: custom_comp={cc} custom_warp={cw} lit={lit}  -> {}", file.display());
    }
}
