//! Microbenchmark for per-pixel evaluation throughput (the hot path).
//!
//! ```text
//! cargo run -p pm-preset --example bench_eval --release
//! ```

use pm_audio::FrameAudioData;
use pm_preset::{FrameParams, Preset};
use std::time::Instant;

fn main() {
    // A representative var-heavy, trig-heavy per-pixel block.
    let milk = "\
per_frame_1=`zoom = 1.0 + 0.1*sin(time);
per_pixel_1=`zoom = zoom + 0.08*sin(rad*10 + time*1.3);
per_pixel_2=`rot = rot + 0.05*cos(ang*3 + time);
per_pixel_3=`dx = 0.02*sin(x*7 + rad*4);
per_pixel_4=`dy = 0.02*cos(y*6 - rad*3);
per_pixel_5=`warp = warp + 0.03*(rad - 0.5)*sin(ang*5);
per_pixel_6=`sx = 1.0 + 0.05*sin(x*3 + time);
per_pixel_7=`cx = 0.5 + 0.1*cos(ang + time*0.5);
";
    let mut preset = Preset::load(milk).expect("load");
    let frame = FrameParams { viewport_width: 1280, viewport_height: 720, mesh_x: 64, mesh_y: 48, ..FrameParams::default() };
    preset.update_frame(frame, FrameAudioData::default()).unwrap();

    // 65 x 49 mesh = 3185 vertices, the projectM default.
    let verts_per_frame = 65 * 49;
    let frames = 400;
    let total = verts_per_frame * frames;

    let start = Instant::now();
    let mut acc = 0.0f64;
    for f in 0..frames {
        for v in 0..verts_per_frame {
            let t = v as f64 / verts_per_frame as f64;
            let out = preset.warp_vertex(t, 1.0 - t, t * 1.4, (t - 0.5) * std::f64::consts::TAU).unwrap();
            acc += out.zoom + out.rot; // prevent optimizing the loop away
        }
        let _ = f;
    }
    let elapsed = start.elapsed();

    let per = elapsed.as_secs_f64() / total as f64;
    println!("per-pixel evals: {total} in {:.3}s", elapsed.as_secs_f64());
    println!("  {:.1} ns/eval, {:.2} M evals/sec", per * 1e9, 1.0 / per / 1e6);
    println!(
        "  budget at 3185 verts/frame: {:.1}% of a 60fps frame (16.7ms)",
        (per * verts_per_frame as f64) / (1.0 / 60.0) * 100.0
    );
    println!("(checksum {acc:.3})");
}
