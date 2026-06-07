//! Renders a full Milkdrop frame headlessly (warp + waveform + composite) and
//! writes a PNG.
//!
//! ```text
//! cargo run -p pm-core --example warp_demo
//! ```
//!
//! Feeds a synthetic audio waveform so the waveform pass injects bright,
//! flowing content into the feedback buffer — the classic Milkdrop look.

use pm_audio::{FrameAudioData, WAVEFORM_SAMPLES};
use pm_core::WarpEngine;
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};

const SIZE: u32 = 600;
const FRAMES: i32 = 240;

/// Synthetic audio for one frame: a moving multi-tone waveform plus beat values.
fn frame_audio(frame: i32) -> FrameAudioData {
    let t = frame as f32 / 30.0;
    let mut audio = FrameAudioData::default();
    for i in 0..WAVEFORM_SAMPLES {
        let p = i as f32 / WAVEFORM_SAMPLES as f32;
        let s = (p * 24.0 + t * 2.0).sin() * 18.0
            + (p * 7.0 - t).sin() * 10.0
            + (p * 53.0 + t * 0.5).sin() * 4.0;
        audio.waveform_left[i] = s;
        audio.waveform_right[i] = s * 0.8 + (p * 11.0 + t).cos() * 6.0;
    }
    // Pulsing beat values.
    audio.bass = 1.0 + 0.8 * (t * 1.5).sin().abs();
    audio.mid = 1.0 + 0.5 * (t * 2.3).sin().abs();
    audio.treb = 1.0 + 0.4 * (t * 3.1).sin().abs();
    audio.vol = (audio.bass + audio.mid + audio.treb) / 3.0;
    audio
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

    // A circular waveform over a gently flowing feedback field.
    let milk = "\
fDecay=0.975
zoom=1.012
rot=0.006
warp=0.9
fWarpScale=1.3
bTexWrap=1
nWaveMode=0
bAdditiveWaves=1
bMaximizeWaveColor=1
fWaveAlpha=1.0
fWaveScale=2.2
wave_r=0.9
wave_g=0.5
wave_b=1.0
per_frame_1=`wave_r = 0.5 + 0.5 * sin(time * 1.3);
per_frame_2=`wave_g = 0.5 + 0.5 * sin(time * 1.7 + 2.0);
per_frame_3=`wave_b = 0.5 + 0.5 * sin(time * 2.3 + 4.0);
per_pixel_1=`rot = rot + 0.15 * (rad - 0.5);
";
    let preset = Preset::load(milk).expect("preset loads");
    let mut engine = WarpEngine::new(&ctx, preset, SIZE, SIZE);

    for frame in 0..FRAMES {
        engine
            .render_frame(&ctx, frame as f32 / 30.0, frame, frame_audio(frame))
            .expect("frame renders");
    }

    let pixels = read_rgba8(&ctx, engine.display_texture());
    let img = image::RgbaImage::from_raw(SIZE, SIZE, pixels).expect("dimensions match");
    let path = "pm-preset-demo.png";
    img.save(path).expect("write PNG");

    let abs = std::env::current_dir().unwrap().join(path);
    println!("Rendered {FRAMES} frames (warp+waveform+composite) to {}", abs.display());
}
