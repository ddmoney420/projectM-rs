//! Render a preset transition at 50% and save the blended frame, to eyeball the
//! crossfade. `cargo run -p pm-core --example transition_demo`

use pm_audio::FrameAudioData;
use pm_core::{PresetPlayer, WarpEngine};
use pm_render::{read_rgba8, GpuContext};

fn audio() -> FrameAudioData {
    let mut a = FrameAudioData::default();
    for i in 0..a.waveform_left.len() {
        let p = i as f32 / 480.0;
        a.waveform_left[i] = (p * 18.0).sin() * 35.0;
        a.waveform_right[i] = (p * 18.0).cos() * 35.0;
    }
    a.bass = 2.0;
    a.mid = 1.5;
    a.treb = 1.0;
    a.vol = 1.5;
    a
}

fn main() {
    let ctx = GpuContext::headless().expect("no GPU");
    // A: warm red circle. B: cool blue line.
    let a = "nWaveMode=0\nfDecay=0.95\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=1.0\nwave_g=0.2\nwave_b=0.1\nfGammaAdj=1.5";
    let b = "nWaveMode=6\nfDecay=0.95\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=0.1\nwave_g=0.4\nwave_b=1.0\nfGammaAdj=1.5";

    let mut player = PresetPlayer::new(&ctx, WarpEngine::new(&ctx, pm_preset::Preset::load(a).unwrap(), 256, 256), 256, 256, 2.0);
    for f in 0..30 {
        player.render(&ctx, f as f32 / 30.0, audio());
    }

    // Switch (transition starts at the last rendered time = 29/30 s).
    player.switch_to(WarpEngine::new(&ctx, pm_preset::Preset::load(b).unwrap(), 256, 256));
    let start = 29.0 / 30.0;
    // Render up to the midpoint (start + 1.0s of a 2.0s window => 50%).
    for f in 30..60 {
        player.render(&ctx, start + (f - 29) as f32 / 30.0, audio());
    }
    println!("transitioning: {}, progress: {:.2}", player.is_transitioning(), player.progress());

    let px = read_rgba8(&ctx, player.output_texture());
    // Save as PNG via the same path render_preset uses (write raw to a file the
    // user can open); here we just report channel means to prove a blend.
    let n = (px.len() / 4) as f64;
    let (mut r, mut g, mut bl) = (0.0, 0.0, 0.0);
    for c in px.chunks_exact(4) {
        r += c[0] as f64;
        g += c[1] as f64;
        bl += c[2] as f64;
    }
    println!("mean RGB = ({:.1}, {:.1}, {:.1}) — a red-only or blue-only frame would be lopsided; a blend shows both", r / n, g / n, bl / n);
    image_write(&px, 256, 256, "pm-transition.png");
    println!("wrote pm-transition.png");
}

fn image_write(rgba: &[u8], w: u32, h: u32, path: &str) {
    // Minimal PNG via the `image` crate if available; else dump raw. pm-core's
    // render_preset uses `image`, so reuse it here.
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).expect("image");
    img.save(path).expect("save png");
}
