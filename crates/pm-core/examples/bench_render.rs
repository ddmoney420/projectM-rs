//! Headless render benchmark: measures per-frame CPU and GPU cost for a set of
//! representative presets, including a preset crossfade (two presets at once).
//!
//! ```text
//! cargo run -p pm-core --example bench_render --release
//! ```
//!
//! Two clocks per frame: **CPU** is how long `render`/`render_frame` takes to
//! return (eval + per-pixel mesh + command encoding + submit; the GPU runs
//! async). **Total** adds a `poll(Wait)` that blocks until the GPU is idle, so
//! **GPU ≈ total − CPU**. 60 fps needs total ≤ 16.67 ms.

use pm_audio::FrameAudioData;
use pm_core::{PresetPlayer, WarpEngine};
use pm_preset::Preset;
use pm_render::{wgpu, GpuContext};

const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const FRAMES: usize = 240;
const WARMUP: usize = 30;

fn wait(ctx: &GpuContext) {
    let _ = ctx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
}

fn audio(frame: usize) -> FrameAudioData {
    let mut a = FrameAudioData::default();
    for i in 0..a.waveform_left.len() {
        let p = (i + frame) as f32 / 480.0;
        a.waveform_left[i] = (p * 17.0).sin() * 30.0;
        a.waveform_right[i] = (p * 17.0).cos() * 30.0;
    }
    a.bass = 1.5 + (frame as f32 * 0.05).sin();
    a.mid = 1.2;
    a.treb = 1.0;
    a.vol = 1.4;
    a
}

struct Stats {
    cpu_ms: Vec<f64>,
    total_ms: Vec<f64>,
}

impl Stats {
    fn pct(sorted: &[f64], p: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx]
    }
    fn report(&self, name: &str) {
        let mut cpu = self.cpu_ms.clone();
        let mut total = self.total_ms.clone();
        cpu.sort_by(|a, b| a.partial_cmp(b).unwrap());
        total.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let avg_cpu = avg(&self.cpu_ms);
        let avg_total = avg(&self.total_ms);
        let p50 = Self::pct(&total, 50.0);
        let p99 = Self::pct(&total, 99.0);
        let max = total.last().copied().unwrap_or(0.0);
        let gpu = (avg_total - avg_cpu).max(0.0);
        let fps = if avg_total > 0.0 { 1000.0 / avg_total } else { 0.0 };
        let verdict = if p99 <= 16.67 {
            "60fps OK"
        } else if p99 <= 33.3 {
            "30-60fps"
        } else {
            "<30fps"
        };
        println!(
            "{name:<28} total avg {avg_total:6.2}ms (cpu {avg_cpu:5.2} / gpu {gpu:5.2}) \
             p50 {p50:5.2} p99 {p99:6.2} max {max:6.2}  ~{fps:5.1}fps  [{verdict}]"
        );
    }
}

/// Render `frames` of a single engine, timing CPU (return) and total (+ GPU wait).
fn bench_engine(ctx: &GpuContext, name: &str, src: &str) -> Stats {
    let mut engine = WarpEngine::new(ctx, Preset::load(src).unwrap(), WIDTH, HEIGHT);
    let mut stats = Stats { cpu_ms: Vec::new(), total_ms: Vec::new() };
    for f in 0..(FRAMES + WARMUP) {
        let t = std::time::Instant::now();
        let _ = engine.render_frame(ctx, f as f32 / 60.0, f as i32, audio(f));
        let cpu = t.elapsed().as_secs_f64() * 1000.0;
        wait(ctx);
        let total = t.elapsed().as_secs_f64() * 1000.0;
        if f >= WARMUP {
            stats.cpu_ms.push(cpu);
            stats.total_ms.push(total);
        }
    }
    stats.report(name);
    stats
}

/// Render a transition: warm preset A, switch to B, time the crossfade frames.
fn bench_transition(ctx: &GpuContext, name: &str, a: &str, b: &str) -> Stats {
    let engine = WarpEngine::new(ctx, Preset::load(a).unwrap(), WIDTH, HEIGHT);
    let mut player = PresetPlayer::new(ctx, engine, WIDTH, HEIGHT, 2.0);
    // Warm A.
    for f in 0..WARMUP {
        player.render(ctx, f as f32 / 60.0, audio(f));
        wait(ctx);
    }
    // Start the crossfade and time it (both presets render + blend each frame).
    player.switch_to(WarpEngine::new(ctx, Preset::load(b).unwrap(), WIDTH, HEIGHT));
    let mut stats = Stats { cpu_ms: Vec::new(), total_ms: Vec::new() };
    let start = WARMUP as f32 / 60.0;
    let mut f = WARMUP;
    // Keep time within the 2.0s window so both presets stay alive.
    while stats.total_ms.len() < FRAMES {
        let time = start + (f - WARMUP) as f32 * 0.004; // ~0.004s steps stay < 2s
        let t = std::time::Instant::now();
        player.render(ctx, time, audio(f));
        let cpu = t.elapsed().as_secs_f64() * 1000.0;
        wait(ctx);
        let total = t.elapsed().as_secs_f64() * 1000.0;
        stats.cpu_ms.push(cpu);
        stats.total_ms.push(total);
        f += 1;
        if !player.is_transitioning() {
            break;
        }
    }
    stats.report(name);
    stats
}

fn main() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; cannot benchmark");
        std::process::exit(1);
    };
    println!("Adapter: {}", ctx.adapter.get_info().name);
    println!("Resolution: {WIDTH}x{HEIGHT}, {FRAMES} frames each (after {WARMUP} warmup)\n");

    // Simple: minimal eval, one circle waveform.
    let simple = "nWaveMode=0\nfDecay=0.96\nfWaveScale=1.5\nbAdditiveWaves=1";

    // Heavy warp: per-pixel code runs the eval over every mesh vertex (64x48).
    let heavy_warp = "\
fDecay=0.97
zoom=1.01
warp=1.0
per_frame_1=`rot = rot + 0.02*sin(time);
per_pixel_1=`zoom = zoom + 0.05*sin(rad*20 + time); rot = rot + 0.1*cos(ang*3); dx = 0.01*sin(y*10); dy = 0.01*cos(x*10);
";

    // Heavy waveform + shapes: custom shapes (many sides) + custom waveform.
    let heavy_shapes = "\
fDecay=0.96
nWaveMode=2
bAdditiveWaves=1
fWaveScale=2.0
wavecode_0_enabled=1
wavecode_0_samples=480
wavecode_0_additive=1
wave_0_per_point1=`x = 0.5 + 0.4*sin(t*6.28 + time); y = 0.5 + 0.4*cos(t*6.28*3 + time);
shapecode_0_enabled=1
shapecode_0_sides=100
shapecode_0_a=0.5
shapecode_1_enabled=1
shapecode_1_sides=100
shapecode_1_a=0.5
shape_0_per_frame1=`x = 0.5 + 0.3*sin(time); r = 0.5; g = 0.7; b = 1.0;
shape_1_per_frame1=`x = 0.5 + 0.3*cos(time*1.3); r = 1.0; g = 0.5; b = 0.2;
";

    // Heavy shader: custom warp + composite shaders sampling noise + blur.
    let heavy_shader = "\
MILKDROP_PRESET_VERSION=201
fDecay=0.97
nWaveMode=0
bAdditiveWaves=1
warp_1=`shader_body
warp_2=`{
warp_3=`float3 n = tex2D(sampler_noise_lq, uv*4 + time*0.1).xyz;
warp_4=`float3 b = GetBlur1(uv);
warp_5=`ret = tex2D(sampler_main, uv + 0.01*(n.xy-0.5)).xyz*0.97 + 0.05*b;
warp_6=`}
comp_1=`shader_body
comp_2=`{
comp_3=`float3 c = tex2D(sampler_main, uv).xyz;
comp_4=`float3 b = GetBlur2(uv);
comp_5=`ret = c + 0.3*b + 0.05*tex2D(sampler_noise_mq, uv*8).xyz;
comp_6=`}
";

    bench_engine(&ctx, "simple", simple);
    bench_engine(&ctx, "heavy-warp (per-pixel)", heavy_warp);
    bench_engine(&ctx, "heavy-waveform+shapes", heavy_shapes);
    bench_engine(&ctx, "heavy-shader (warp+comp)", heavy_shader);
    println!();
    bench_transition(&ctx, "transition: warp<->shader", heavy_warp, heavy_shader);
    bench_transition(&ctx, "transition: shapes<->shader", heavy_shapes, heavy_shader);
}
