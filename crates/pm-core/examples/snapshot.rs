//! Renderer regression snapshots: render a fixed set of presets with fixed
//! inputs and compare against committed baseline PNGs. These catch visual
//! regressions in *our own* renderer — this is NOT C++/projectM parity.
//!
//! ```text
//! cargo run -p pm-core --example snapshot --release            # check
//! cargo run -p pm-core --example snapshot --release -- --update # regenerate
//! PM_SNAPSHOT_UPDATE=1 cargo run -p pm-core --example snapshot  # regenerate
//! ```
//!
//! Baselines live in `crates/pm-core/tests/snapshots/<name>.png`. On mismatch,
//! the actual + amplified-diff images are written to `target/snapshot-out/` and
//! the run exits non-zero. Updating baselines is deliberate (`--update`), never
//! automatic. Comparison uses a small tolerance so minor GPU/driver variance
//! doesn't trip the check; a real regression moves many pixels well past it.

use pm_audio::FrameAudioData;
use pm_core::{PresetPlayer, WarpEngine};
use pm_preset::Preset;
use pm_render::{read_rgba8, GpuContext};
use std::path::PathBuf;

const SIZE: u32 = 256;
const FRAMES: usize = 24;

// Tolerance: a pixel "changed" if any channel differs by more than this; the
// check fails if too many pixels change or the mean delta is too high.
const CHANNEL_TOL: u8 = 4;
const MAX_CHANGED_FRACTION: f64 = 0.01;
const MAX_MEAN_DELTA: f64 = 1.0;

enum Scenario {
    Single(&'static str),
    /// A crossfade captured at exactly 50% progress.
    Transition(&'static str, &'static str),
}

fn scenarios() -> Vec<(&'static str, Scenario)> {
    vec![
        ("simple_warp", Scenario::Single(
            "nWaveMode=0\nfDecay=0.96\nfWaveScale=1.5\nbAdditiveWaves=1\nzoom=1.01\nrot=0.03",
        )),
        ("waveform_heavy", Scenario::Single(
            "nWaveMode=2\nfDecay=0.95\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=0.3\nwave_g=0.8\nwave_b=1.0",
        )),
        ("custom_shape", Scenario::Single(
            "fDecay=0.96\nnWaveMode=0\nbAdditiveWaves=1\nshapecode_0_enabled=1\nshapecode_0_sides=6\nshapecode_0_a=0.8\nshapecode_0_r=1.0\nshapecode_0_g=0.4\nshapecode_0_b=0.1\nshape_0_per_frame1=`x=0.5;y=0.5;rad=0.35;ang=time;",
        )),
        ("textured_shape", Scenario::Single(
            "nWaveMode=2\nfDecay=0.96\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=1.0\nwave_g=1.0\nwave_b=1.0\nzoom=1.01\nshapecode_0_enabled=1\nshapecode_0_sides=4\nshapecode_0_textured=1\nshapecode_0_tex_zoom=0.7\nshapecode_0_a=1.0\nshapecode_0_r=1.0\nshapecode_0_g=1.0\nshapecode_0_b=1.0\nshape_0_per_frame1=`x=0.5;y=0.5;rad=0.4;ang=time;tex_ang=time*0.5;",
        )),
        ("motion_vectors", Scenario::Single(
            "zoom=1.05\nrot=0.08\nwarp=0.2\nfDecay=0.95\nnWaveMode=0\nmv_x=16\nmv_y=12\nmv_l=2.0\nmv_r=1.0\nmv_g=1.0\nmv_b=0.3\nmv_a=1.0\nfVideoEchoAlpha=0.0",
        )),
        ("transition_50pct", Scenario::Transition(
            "nWaveMode=0\nfDecay=0.95\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=1.0\nwave_g=0.2\nwave_b=0.1",
            "nWaveMode=6\nfDecay=0.95\nbAdditiveWaves=1\nfWaveScale=2.0\nwave_r=0.1\nwave_g=0.4\nwave_b=1.0",
        )),
    ]
}

/// Deterministic per-frame audio (no wall clock, no RNG).
fn snap_audio(frame: usize) -> FrameAudioData {
    let mut a = FrameAudioData::default();
    for i in 0..a.waveform_left.len() {
        let p = (i + frame * 7) as f32 / 480.0;
        a.waveform_left[i] = (p * 16.0).sin() * 32.0;
        a.waveform_right[i] = (p * 16.0).cos() * 32.0;
    }
    a.bass = 1.6;
    a.mid = 1.3;
    a.treb = 1.0;
    a.vol = 1.4;
    a
}

fn render_single(ctx: &GpuContext, milk: &str) -> Vec<u8> {
    let mut engine = WarpEngine::new(ctx, Preset::load(milk).unwrap(), SIZE, SIZE);
    engine.seed(ctx, &vec![0u8; (SIZE * SIZE * 4) as usize]);
    for f in 0..FRAMES {
        engine.render_frame(ctx, f as f32 / 30.0, f as i32, snap_audio(f)).unwrap();
    }
    read_rgba8(ctx, engine.display_texture())
}

/// Render a crossfade and capture the frame at exactly 50% progress.
fn render_transition(ctx: &GpuContext, a: &str, b: &str) -> Vec<u8> {
    const WARM: usize = 16;
    const DURATION: f32 = 2.0;
    let engine = WarpEngine::new(ctx, Preset::load(a).unwrap(), SIZE, SIZE);
    let mut player = PresetPlayer::new(ctx, engine, SIZE, SIZE, DURATION);
    player.render(ctx, 0.0, snap_audio(0)); // establish last_time = 0.0
    for f in 1..WARM {
        player.render(ctx, f as f32 / 30.0, snap_audio(f));
    }
    // Transition starts at the current last_time = (WARM-1)/30.
    let start = (WARM - 1) as f32 / 30.0;
    player.switch_to(WarpEngine::new(ctx, Preset::load(b).unwrap(), SIZE, SIZE));
    // Advance in 30 steps of 1/30s; the last lands at start + 1.0s = 50% of 2s.
    for k in 1..=30 {
        player.render(ctx, start + k as f32 / 30.0, snap_audio(WARM + k));
    }
    assert!((player.progress() - 0.5).abs() < 1e-4, "expected 50% progress, got {}", player.progress());
    read_rgba8(ctx, player.output_texture())
}

struct Metrics {
    max_delta: u8,
    mean_delta: f64,
    changed: usize,
    total: usize,
}

impl Metrics {
    fn passes(&self) -> bool {
        let frac = self.changed as f64 / self.total.max(1) as f64;
        frac <= MAX_CHANGED_FRACTION && self.mean_delta <= MAX_MEAN_DELTA
    }
}

fn compare(expected: &[u8], actual: &[u8]) -> Metrics {
    let total = actual.len() / 4;
    let (mut max_delta, mut sum, mut changed) = (0u8, 0u64, 0usize);
    for (e, a) in expected.chunks_exact(4).zip(actual.chunks_exact(4)) {
        let mut pixel_changed = false;
        for c in 0..4 {
            let d = e[c].abs_diff(a[c]);
            max_delta = max_delta.max(d);
            sum += d as u64;
            if d > CHANNEL_TOL {
                pixel_changed = true;
            }
        }
        if pixel_changed {
            changed += 1;
        }
    }
    Metrics { max_delta, mean_delta: sum as f64 / (total * 4).max(1) as f64, changed, total }
}

/// Amplified absolute-difference image for eyeballing where things changed.
fn diff_image(expected: &[u8], actual: &[u8]) -> Vec<u8> {
    expected
        .chunks_exact(4)
        .zip(actual.chunks_exact(4))
        .flat_map(|(e, a)| {
            let d = |i: usize| (e[i].abs_diff(a[i]) as u32 * 8).min(255) as u8;
            [d(0), d(1), d(2), 255]
        })
        .collect()
}

fn baseline_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("snapshots")
}

fn save(path: &std::path::Path, rgba: &[u8]) {
    let img = image::RgbaImage::from_raw(SIZE, SIZE, rgba.to_vec()).expect("image");
    img.save(path).expect("save png");
}

fn main() {
    let update = std::env::args().any(|a| a == "--update") || std::env::var_os("PM_SNAPSHOT_UPDATE").is_some();

    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping snapshot run (exit 0)");
        return;
    };
    println!("Adapter: {}  ({SIZE}x{SIZE}, {FRAMES} frames)", ctx.adapter.get_info().name);

    let dir = baseline_dir();
    std::fs::create_dir_all(&dir).expect("create baseline dir");
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("snapshot-out");

    let mut failures = 0;
    for (name, scenario) in scenarios() {
        let actual = match scenario {
            Scenario::Single(milk) => render_single(&ctx, milk),
            Scenario::Transition(a, b) => render_transition(&ctx, a, b),
        };
        let baseline = dir.join(format!("{name}.png"));

        if update {
            save(&baseline, &actual);
            println!("  updated  {name}");
            continue;
        }

        let Ok(img) = image::open(&baseline) else {
            println!("  MISSING  {name}  (run with --update to create the baseline)");
            failures += 1;
            continue;
        };
        let expected = img.to_rgba8().into_raw();
        if expected.len() != actual.len() {
            println!("  SIZE-DIFF {name}  baseline {} vs actual {} bytes", expected.len(), actual.len());
            failures += 1;
            continue;
        }
        let m = compare(&expected, &actual);
        if m.passes() {
            println!("  ok       {name}  (max {} mean {:.3} changed {}/{})", m.max_delta, m.mean_delta, m.changed, m.total);
        } else {
            failures += 1;
            std::fs::create_dir_all(&out_dir).ok();
            save(&out_dir.join(format!("{name}.actual.png")), &actual);
            save(&out_dir.join(format!("{name}.diff.png")), &diff_image(&expected, &actual));
            println!(
                "  FAIL     {name}  max {} mean {:.3} changed {}/{} ({:.2}%) -> target/snapshot-out/{name}.{{actual,diff}}.png",
                m.max_delta, m.mean_delta, m.changed, m.total, 100.0 * m.changed as f64 / m.total as f64,
            );
        }
    }

    if update {
        println!("baselines updated.");
    } else if failures > 0 {
        eprintln!("\n{failures} snapshot(s) failed.");
        std::process::exit(1);
    } else {
        println!("all snapshots match.");
    }
}
