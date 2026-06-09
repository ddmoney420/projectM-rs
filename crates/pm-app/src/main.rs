//! `pm-app` — a live window that renders Milkdrop presets reacting to audio.
//!
//! ```text
//! cargo run -p pm-app --release -- [preset-dir]
//! ```
//!
//! Keys: →/Space/N next preset · ←/P previous · R random · F5/L reload current ·
//! T toggle transitions · F toggle perf overlay · H toggle HUD · Pause/K
//! freeze (`.` step one frame while paused) · A auto-advance ([ / ] adjust
//! interval) · S shuffle · Esc/Q quit.
//!
//! HUD/transitions/perf/auto-advance(+interval)/shuffle preferences persist
//! across launches (see `prefs`). Env: `PM_PERF` forces the perf overlay on at
//! launch (overriding the saved pref, without changing it); `PM_SCAN` prints a
//! one-time corpus compatibility summary at startup (never persisted).

mod audio;
mod blit;
mod hud;
mod prefs;

use audio::AudioInput;
use blit::Blit;
use hud::Hud;
use prefs::Prefs;
use pm_core::{PresetPlayer, WarpEngine, DEFAULT_TRANSITION_SECS};
use pm_preset::{shader_to_wgsl, Preset, ShaderKind};
use pm_render::wgpu;
use pm_render::{read_rgba8, GpuContext};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

const DEFAULT_DIR: &str = r"C:\My-Workspace\projectm-presets\cream-of-the-crop";

/// A fallback preset used when no preset files are found.
const BUILTIN: &str = "\
fDecay=0.975
zoom=1.012
rot=0.006
warp=0.9
bTexWrap=1
nWaveMode=0
bAdditiveWaves=1
fWaveAlpha=1.0
fWaveScale=2.0
per_frame_1=`wave_r = 0.5 + 0.5*sin(time*1.3);
per_frame_2=`wave_g = 0.5 + 0.5*sin(time*1.7 + 2);
per_frame_3=`wave_b = 0.5 + 0.5*sin(time*2.3 + 4);
per_pixel_1=`rot = rot + 0.15*(rad - 0.5);
";

struct Render {
    window: Arc<Window>,
    ctx: GpuContext,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    player: PresetPlayer,
    blit: Blit,
    hud: Hud,
    /// Current preset's display name (for the HUD).
    name: String,
}

/// Auto-advance: adjust in 5 s steps, clamped to 5..600 s (default lives in
/// `prefs::Prefs::default`).
const AUTO_STEP_SECS: f32 = 5.0;
const AUTO_MIN_SECS: f32 = 5.0;
const AUTO_MAX_SECS: f32 = 600.0;

/// Recent-history length for shuffle no-repeat avoidance.
const SHUFFLE_HISTORY: usize = 25;

/// Time increment for one stepped frame while paused (a normal ~60fps frame).
const STEP_DT: f32 = 1.0 / 60.0;

/// Number of warm-up frames to build feedback content before probing.
const WARMUP_FRAMES: i32 = 8;
/// Max presets to skip when searching for one that renders visible content.
const MAX_PROBE: usize = 400;
/// A preset "renders" if at least this many display pixels are non-black.
const CONTENT_THRESHOLD: usize = 400;

struct App {
    presets: Vec<PathBuf>,
    index: usize,
    /// False until the user navigates into the corpus (we start on the built-in).
    on_corpus: bool,
    rng: u32,
    audio: AudioInput,
    render: Option<Render>,
    start: Instant,
    last: Instant,
    frame: i32,
    /// Frame-time logging, enabled with the `PM_PERF` env var (toggle with `F`).
    perf: Option<Perf>,
    /// Cumulative navigation stats, reported on exit.
    skipped_black: usize,
    skipped_unparsed: usize,
    shown: usize,
    /// In-window HUD overlay visibility (toggle with `H`). Default on.
    hud_visible: bool,
    /// Frozen state (toggle with `Pause`/`K`): preset time + feedback stop
    /// advancing while the window keeps presenting the last frame.
    paused: bool,
    /// Render exactly one more frame even while paused (set after a preset
    /// change so the new selection becomes visible, then frozen).
    force_frame: bool,
    /// User-requested single-frame step while paused (advances preset time +
    /// feedback by one frame, then re-freezes).
    step: bool,
    /// Held preset `time` (seconds) while paused, advanced by `STEP_DT` per step.
    frozen_time: f32,
    /// Timed auto-advance (toggle `A`; interval adjusted with `[` / `]`).
    auto_advance: bool,
    auto_interval: f32,
    /// Wall-seconds elapsed toward the next auto-advance (paused-aware).
    auto_elapsed: f32,
    /// Shuffle mode (toggle `S`): random, no-repeat selection for auto-advance.
    shuffle: bool,
    /// Recently shown preset indices (most-recent first) for no-repeat.
    history: VecDeque<usize>,
    /// Transitions enabled (mirrors the player's duration; persisted).
    transitions_on: bool,
    /// The user's persisted perf preference (distinct from the runtime overlay,
    /// which `PM_PERF` can force on at launch without changing this).
    perf_pref: bool,
    /// Where persisted preferences live.
    prefs_path: PathBuf,
}

/// Rolling per-second frame-timing accumulator (opt-in via `PM_PERF`).
struct Perf {
    frames: u32,
    render_ms: f64,
    present_ms: f64,
    worst_ms: f64,
    transition_frames: u32,
    last_print: Instant,
}

impl Perf {
    fn new() -> Self {
        Perf {
            frames: 0,
            render_ms: 0.0,
            present_ms: 0.0,
            worst_ms: 0.0,
            transition_frames: 0,
            last_print: Instant::now(),
        }
    }

    /// Record one frame; print a rolling summary about once a second.
    fn tick(&mut self, render_ms: f64, present_ms: f64, transitioning: bool) {
        self.frames += 1;
        self.render_ms += render_ms;
        self.present_ms += present_ms;
        self.worst_ms = self.worst_ms.max(render_ms + present_ms);
        if transitioning {
            self.transition_frames += 1;
        }
        let elapsed = self.last_print.elapsed().as_secs_f64();
        if elapsed >= 1.0 && self.frames > 0 {
            let n = self.frames as f64;
            let frame_ms = (self.render_ms + self.present_ms) / n;
            println!(
                "[perf] {:.1} fps  frame avg {:.2}ms (render {:.2} / present {:.2})  worst {:.2}ms  {} transition frames",
                self.frames as f64 / elapsed,
                frame_ms,
                self.render_ms / n,
                self.present_ms / n,
                self.worst_ms,
                self.transition_frames,
            );
            *self = Perf::new();
        }
    }
}

impl App {
    fn new(presets: Vec<PathBuf>, prefs: Prefs, prefs_path: PathBuf) -> Self {
        let now = Instant::now();
        // `PM_PERF` forces the overlay on at launch even when the saved pref is
        // off; it does not change the persisted preference.
        let perf_env = std::env::var_os("PM_PERF").is_some();
        let perf_on = prefs.perf || perf_env;
        App {
            presets,
            index: 0,
            on_corpus: false,
            rng: 0x1234_5678,
            audio: AudioInput::new(),
            render: None,
            start: now,
            last: now,
            frame: 0,
            perf: perf_on.then(Perf::new),
            skipped_black: 0,
            skipped_unparsed: 0,
            shown: 0,
            hud_visible: prefs.hud,
            paused: false,
            force_frame: false,
            step: false,
            frozen_time: 0.0,
            auto_advance: prefs.auto,
            auto_interval: prefs.auto_interval.clamp(AUTO_MIN_SECS, AUTO_MAX_SECS),
            auto_elapsed: 0.0,
            shuffle: prefs.shuffle,
            history: VecDeque::new(),
            transitions_on: prefs.transitions,
            perf_pref: prefs.perf,
            prefs_path,
        }
    }

    /// Persist the current preference subset (called when a pref toggles).
    fn save_prefs(&self) {
        Prefs {
            hud: self.hud_visible,
            transitions: self.transitions_on,
            perf: self.perf_pref,
            auto: self.auto_advance,
            auto_interval: self.auto_interval,
            shuffle: self.shuffle,
        }
        .save(&self.prefs_path);
    }

    /// Advance the LCG and return an index in `0..len`.
    fn next_rng(&mut self, len: usize) -> usize {
        self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        (self.rng as usize) % len.max(1)
    }

    /// Pick a random start index avoiding the current preset and recent history;
    /// degrades to any index != current when the corpus is small.
    fn pick_shuffle_start(&mut self, len: usize) -> usize {
        if len <= 1 {
            return 0;
        }
        for _ in 0..16 {
            let i = self.next_rng(len);
            if i != self.index && !self.history.contains(&i) {
                return i;
            }
        }
        let i = self.next_rng(len);
        if i == self.index {
            (i + 1) % len
        } else {
            i
        }
    }

    /// Record the shown preset in the no-repeat history (skipped ones excluded).
    fn remember(&mut self, idx: usize, len: usize) {
        self.history.push_front(idx);
        let cap = SHUFFLE_HISTORY.min(len.saturating_sub(1));
        while self.history.len() > cap {
            self.history.pop_back();
        }
    }

    /// Build the HUD text lines from current runtime state.
    fn hud_lines(&self) -> Vec<String> {
        let Some(render) = &self.render else { return Vec::new() };
        let total = self.presets.len().max(1);
        let mut lines = vec![render.name.clone()];
        if self.paused {
            lines.push("PAUSED · STEP .".to_string());
        }
        let mut status = format!("[{}/{}]", self.index + 1, total);
        if render.player.is_transitioning() {
            status.push_str(" XFADE");
        } else if render.player.duration() <= 0.0 {
            status.push_str(" CUT");
        }
        if self.shuffle {
            status.push_str(" SHUF");
        }
        lines.push(status);
        if self.auto_advance {
            let remaining = (self.auto_interval - self.auto_elapsed).max(0.0).ceil() as u32;
            lines.push(format!("AUTO {remaining}S / {:.0}S", self.auto_interval));
        }
        if self.skipped_black + self.skipped_unparsed > 0 {
            lines.push(format!("SKIPPED {} / {} UNPARSED", self.skipped_black, self.skipped_unparsed));
        }
        if self.perf.is_some() {
            lines.push("PERF ON (SEE CONSOLE)".to_string());
        }
        lines
    }

    /// The current preset: the built-in until the user navigates into the corpus.
    fn current_preset(&self) -> (Preset, String) {
        if self.on_corpus {
            if let Some(path) = self.presets.get(self.index) {
                if let Ok(bytes) = std::fs::read(path) {
                    if let Ok(preset) = Preset::load(&String::from_utf8_lossy(&bytes)) {
                        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                        return (preset, name);
                    }
                }
            }
        }
        (Preset::load(BUILTIN).expect("builtin preset"), "built-in".into())
    }

    /// Navigate the corpus, skipping presets that render black (many advanced
    /// presets need content generators we don't render yet).
    fn change_preset(&mut self, delta: i64, random: bool) {
        if self.presets.is_empty() {
            return;
        }
        let len = self.presets.len();
        let dir: i64 = if delta < 0 { -1 } else { 1 };
        let start = if random {
            self.pick_shuffle_start(len)
        } else if !self.on_corpus {
            // First step into the corpus: begin at an end.
            if dir > 0 { 0 } else { len - 1 }
        } else {
            ((self.index as i64 + delta).rem_euclid(len as i64)) as usize
        };
        self.on_corpus = true;

        let Some(render) = self.render.as_mut() else { return };
        let (w, h) = (render.config.width, render.config.height);
        let found =
            find_renderable(&render.ctx, w, h, &self.presets, &mut self.audio, self.frame, start, dir);
        let Found { engine, idx, name, black, unparsed, fell_back } = found;
        let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
        if self.paused {
            // Frozen: hard-cut to the new preset (a crossfade can't advance while
            // paused) and request one frame so it becomes visible, then frozen.
            let d = render.player.duration();
            render.player.set_duration(0.0);
            render.player.switch_to(engine);
            render.player.set_duration(d);
            self.force_frame = true;
        } else {
            render.player.switch_to(engine);
        }
        let Some(render) = self.render.as_mut() else { return };
        render.name = name.clone();
        render.window.set_title(&format!("pm-app — {name}{cc}  [{}/{}]", idx + 1, len));

        // Tally + log why candidates were skipped on the way here.
        self.skipped_black += black;
        self.skipped_unparsed += unparsed;
        self.shown += 1;
        let mut why = String::new();
        if black + unparsed > 0 {
            why = format!(" (skipped {}: {black} black", black + unparsed);
            if unparsed > 0 {
                why.push_str(&format!(", {unparsed} unparsed"));
            }
            why.push(')');
        }
        if fell_back {
            println!("Showing [{}/{}]: built-in (no renderable preset found nearby){why}", idx + 1, len);
        } else {
            println!("Showing [{}/{}]: {name}{why}", idx + 1, len);
        }
        self.index = idx;
        self.remember(idx, len); // record only the shown preset (not skipped ones)
        self.auto_elapsed = 0.0; // newly shown preset gets a full interval
    }

    /// Toggle timed auto-advance; reset the timer so the change is predictable.
    fn toggle_auto(&mut self) {
        self.auto_advance = !self.auto_advance;
        self.auto_elapsed = 0.0;
        println!(
            "Auto-advance: {}",
            if self.auto_advance { format!("on (every {:.0}s)", self.auto_interval) } else { "off".into() }
        );
        self.save_prefs();
    }

    /// Toggle shuffle mode (random, no-repeat selection for auto-advance / R).
    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        println!("Shuffle: {}", if self.shuffle { "on" } else { "off" });
        self.save_prefs();
    }

    /// Adjust the auto-advance interval by `delta` seconds, clamped.
    fn adjust_auto_interval(&mut self, delta: f32) {
        self.auto_interval = (self.auto_interval + delta).clamp(AUTO_MIN_SECS, AUTO_MAX_SECS);
        if self.auto_elapsed > self.auto_interval {
            self.auto_elapsed = self.auto_interval;
        }
        println!("Auto-advance interval: {:.0}s", self.auto_interval);
        self.save_prefs();
    }

    /// Toggle smooth preset transitions on/off (off = instant hard cut).
    fn toggle_transitions(&mut self) {
        self.transitions_on = !self.transitions_on;
        let dur = if self.transitions_on { DEFAULT_TRANSITION_SECS } else { 0.0 };
        if let Some(render) = &mut self.render {
            render.player.set_duration(dur);
        }
        println!("Preset transitions: {}", if self.transitions_on { "on (2.7s)" } else { "off (hard cut)" });
        self.save_prefs();
    }

    /// Toggle the in-window HUD overlay.
    fn toggle_hud(&mut self) {
        self.hud_visible = !self.hud_visible;
        println!("HUD: {}", if self.hud_visible { "on" } else { "off" });
        self.save_prefs();
    }

    /// Reload the current preset from disk and restart it (a hard reset, not a
    /// transition) — useful after editing a `.milk` file.
    fn reload_current(&mut self) {
        let (preset, name) = self.current_preset();
        if let Some(render) = &mut self.render {
            let (w, h) = (render.config.width, render.config.height);
            let engine = WarpEngine::new(&render.ctx, preset, w, h);
            let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
            let duration = render.player.duration();
            render.player = PresetPlayer::new(&render.ctx, engine, w, h, duration);
            render.name = name.clone();
            render.window.set_title(&format!("pm-app — {name}{cc}  [{}/{}]", self.index + 1, self.presets.len().max(1)));
            println!("Reloaded: {name}");
        }
        self.force_frame = true; // repaint once even if frozen
        self.auto_elapsed = 0.0; // reload restarts the interval
    }

    /// Toggle the frozen/paused state. While paused the visualizer holds its last
    /// frame; navigation/reload still work and stay frozen until unpaused.
    fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        self.force_frame = true; // refresh the HUD (PAUSED) immediately
        println!("Playback: {}", if self.paused { "PAUSED (frozen)" } else { "running" });
    }

    /// Step exactly one visual frame — only while paused (otherwise a no-op hint).
    fn step_frame(&mut self) {
        if self.paused {
            self.step = true;
        } else {
            println!("Step (.) only works while paused (Pause/K to freeze)");
        }
    }

    /// Toggle the per-second frame-timing overlay (also enabled at launch with
    /// the `PM_PERF` env var).
    fn toggle_perf(&mut self) {
        let on = self.perf.is_none();
        self.perf = on.then(Perf::new);
        self.perf_pref = on; // explicit toggle updates the persisted preference
        println!("Perf overlay: {}", if on { "on (per-second frame timings)" } else { "off" });
        self.save_prefs();
    }

    /// Print the cumulative session stats (called on exit).
    fn log_session(&self) {
        println!(
            "── session ──  presets shown: {}  ·  skipped: {} black, {} unparsed",
            self.shown, self.skipped_black, self.skipped_unparsed
        );
    }

    /// Recreate the warp engine for the current preset at the current size.
    fn rebuild_engine(&mut self) {
        let (preset, name) = self.current_preset();
        if let Some(render) = &mut self.render {
            let (w, h) = (render.config.width, render.config.height);
            let engine = WarpEngine::new(&render.ctx, preset, w, h);
            let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
            // Resize is a clean reset, not a transition.
            let duration = render.player.duration();
            render.player = PresetPlayer::new(&render.ctx, engine, w, h, duration);
            render.name = name.clone();
            render.window.set_title(&format!(
                "pm-app — {name}{cc}  [{}/{}]",
                self.index + 1,
                self.presets.len().max(1)
            ));
        }
        self.force_frame = true; // repaint once even if frozen
    }

    fn render(&mut self) {
        // Advance the clock before borrowing `self.render`.
        let now = Instant::now();
        let wall_dt = now.saturating_duration_since(self.last);
        self.last = now;
        // Preset `time`: while paused it's held at `frozen_time`, advancing by one
        // `STEP_DT` only when the user steps (`.`); `start` is re-anchored so the
        // value is deterministic and resumes seamlessly on unpause.
        let time = if self.paused {
            if self.step {
                self.frozen_time += STEP_DT;
            }
            self.start = now - Duration::from_secs_f32(self.frozen_time);
            self.frozen_time
        } else {
            let t = (now - self.start).as_secs_f32();
            self.frozen_time = t;
            t
        };

        // Auto-advance timer: accumulate only when running (not paused) with a
        // corpus to cycle. `change_preset` resets `auto_elapsed`, so the freshly
        // shown preset always gets a full interval (skipped presets don't count).
        if self.auto_advance && !self.paused && !self.presets.is_empty() {
            self.auto_elapsed += wall_dt.as_secs_f32();
            if self.auto_elapsed >= self.auto_interval {
                // Shuffle picks a random no-repeat preset; otherwise sequential.
                if self.shuffle {
                    self.change_preset(0, true);
                } else {
                    self.change_preset(1, false);
                }
            }
        }

        // Build HUD text after any auto-advance, so it reflects the shown preset.
        let hud_lines = self.hud_visible.then(|| self.hud_lines());

        // Render a frame when running, on the single forced frame after a paused
        // navigation, or on a user step. Otherwise `player.render` is skipped
        // entirely — time, the frame counter, feedback iteration and transitions
        // all hold, and the last frame is simply re-presented.
        let do_render = !self.paused || self.force_frame || self.step;
        // Deterministic one-frame audio delta while paused (step/force), else wall.
        let audio = do_render.then(|| {
            let dt = if self.paused { STEP_DT as f64 } else { wall_dt.as_secs_f64().min(0.1) };
            self.audio.frame_data(dt, self.frame)
        });
        let perf = self.perf.is_some();

        let Some(render) = &mut self.render else { return };

        let mut render_ms = 0.0;
        if let Some(audio) = audio {
            let t_render = perf.then(Instant::now);
            render.player.render(&render.ctx, time, audio);
            render_ms = t_render.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
        }

        let t_present = perf.then(Instant::now);
        match render.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                render.blit.draw(&render.ctx, render.player.output_texture(), &view);
                if let Some(lines) = &hud_lines {
                    render.hud.update(&render.ctx, lines);
                    render.hud.draw(&render.ctx, &view, render.config.width, render.config.height);
                }
                frame.present();
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                render.surface.configure(&render.ctx.device, &render.config);
            }
            // Timeout / Occluded / Validation: skip this frame.
            _ => {}
        }
        let present_ms = t_present.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0);
        let transitioning = render.player.is_transitioning();

        if do_render {
            self.frame += 1;
            self.force_frame = false;
            self.step = false; // a step renders exactly one frame, then re-freezes
            if let Some(p) = &mut self.perf {
                p.tick(render_ms, present_ms, transitioning);
            }
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if let Some(render) = &mut self.render {
            render.config.width = width;
            render.config.height = height;
            render.surface.configure(&render.ctx.device, &render.config);
        }
        // Recreate the engine to match the new resolution.
        self.rebuild_engine();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render.is_some() {
            return;
        }

        let attrs = Window::default_attributes()
            .with_title("pm-app")
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 720.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .expect("no compatible GPU adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("pm-app device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .expect("request device");

        println!("Rendering on: {}", adapter.get_info().name);
        println!("Audio source: {}", self.audio.source());

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let ctx = GpuContext { instance, adapter, device, queue };
        let blit = Blit::new(&ctx, format);
        let hud = Hud::new(&ctx, format);
        let (preset, name) = self.current_preset();
        let engine = WarpEngine::new(&ctx, preset, w, h);
        let cc = if engine.uses_custom_composite() { " ·custom-comp" } else { "" };
        window.set_title(&format!("pm-app — {name}{cc}  [{}/{}]", self.index + 1, self.presets.len().max(1)));
        let duration = if self.transitions_on { DEFAULT_TRANSITION_SECS } else { 0.0 };
        let player = PresetPlayer::new(&ctx, engine, w, h, duration);

        self.render = Some(Render { window, ctx, surface, config, player, blit, hud, name });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.log_session();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.resize(size.width, size.height),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::KeyboardInput {
                event: KeyEvent { logical_key, state: ElementState::Pressed, .. },
                ..
            } => match logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.log_session();
                    event_loop.exit();
                }
                Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Space) => self.change_preset(1, false),
                Key::Named(NamedKey::ArrowLeft) => self.change_preset(-1, false),
                Key::Named(NamedKey::F5) => self.reload_current(),
                Key::Named(NamedKey::Pause) => self.toggle_pause(),
                Key::Character(c) => match c.as_str() {
                    "n" => self.change_preset(1, false),
                    "p" => self.change_preset(-1, false),
                    "r" => self.change_preset(0, true),
                    "t" => self.toggle_transitions(),
                    "f" => self.toggle_perf(),
                    "h" => self.toggle_hud(),
                    "k" => self.toggle_pause(),
                    "a" => self.toggle_auto(),
                    "s" => self.toggle_shuffle(),
                    "." => self.step_frame(),
                    "[" => self.adjust_auto_interval(-AUTO_STEP_SECS),
                    "]" => self.adjust_auto_interval(AUTO_STEP_SECS),
                    "l" => self.reload_current(),
                    "q" => {
                        self.log_session();
                        event_loop.exit();
                    }
                    _ => {}
                },
                _ => {}
            },
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(render) = &self.render {
            render.window.request_redraw();
        }
    }
}

/// Load a preset file, returning the parsed preset and its display name.
fn load_preset(path: &Path) -> Option<(Preset, String)> {
    let bytes = std::fs::read(path).ok()?;
    let preset = Preset::load(&String::from_utf8_lossy(&bytes)).ok()?;
    let name = path.file_name()?.to_string_lossy().into_owned();
    Some((preset, name))
}

/// Run a few frames so the feedback buffer accumulates content.
fn warm_up(engine: &mut WarpEngine, ctx: &GpuContext, audio: &mut AudioInput, base: i32) {
    for f in 0..WARMUP_FRAMES {
        let n = base + f;
        let a = audio.frame_data(1.0 / 60.0, n);
        let _ = engine.render_frame(ctx, n as f32 / 60.0, n, a);
    }
}

/// True if the engine's display output has visible (non-black) content.
fn has_content(ctx: &GpuContext, engine: &WarpEngine) -> bool {
    let px = read_rgba8(ctx, engine.display_texture());
    px.chunks_exact(4)
        .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30)
        .count()
        > CONTENT_THRESHOLD
}

/// Resolution used for the cheap "does it render anything?" probe.
const PROBE_SIZE: u32 = 160;

/// Search from `start` in direction `dir` for a preset that renders visible
/// content. Each candidate is probed at low resolution (fast); the winner is
/// rebuilt at full resolution. Falls back to the built-in if none found.
/// Outcome of a navigation probe: the chosen engine plus how many candidates
/// were skipped and why, so the caller can log it.
struct Found {
    engine: WarpEngine,
    idx: usize,
    name: String,
    /// Candidates that rendered black (need generators we don't support).
    black: usize,
    /// Candidates that couldn't be read or parsed.
    unparsed: usize,
    /// True when no candidate rendered and we fell back to the built-in.
    fell_back: bool,
}

#[allow(clippy::too_many_arguments)]
fn find_renderable(
    ctx: &GpuContext,
    w: u32,
    h: u32,
    presets: &[PathBuf],
    audio: &mut AudioInput,
    frame_base: i32,
    start: usize,
    dir: i64,
) -> Found {
    let len = presets.len();
    let (mut black, mut unparsed) = (0usize, 0usize);
    for attempt in 0..MAX_PROBE.min(len) {
        let idx = ((start as i64 + dir * attempt as i64).rem_euclid(len as i64)) as usize;
        let (preset, name) = match load_preset(&presets[idx]) {
            Some(p) => p,
            None => {
                unparsed += 1;
                let n = presets[idx].file_name().unwrap_or_default().to_string_lossy();
                eprintln!("  skip (unparsed): {n}");
                continue;
            }
        };

        let mut probe = WarpEngine::new(ctx, preset, PROBE_SIZE, PROBE_SIZE);
        warm_up(&mut probe, ctx, audio, frame_base);
        if has_content(ctx, &probe) {
            // Rebuild the winner at full resolution.
            let (preset, _) = load_preset(&presets[idx]).expect("reload");
            let mut engine = WarpEngine::new(ctx, preset, w, h);
            warm_up(&mut engine, ctx, audio, frame_base);
            return Found { engine, idx, name, black, unparsed, fell_back: false };
        }
        black += 1;
    }
    // Nothing rendered in range — show the built-in instead.
    let mut engine = WarpEngine::new(ctx, Preset::load(BUILTIN).unwrap(), w, h);
    warm_up(&mut engine, ctx, audio, frame_base);
    Found { engine, idx: start.min(len.saturating_sub(1)), name: "built-in".into(), black, unparsed, fell_back: true }
}

fn collect_presets(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_presets(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("milk")) {
            out.push(path);
        }
    }
}

/// CPU-only compatibility scan (opt-in via `PM_SCAN`): parse every preset and
/// translate its custom shaders, reporting load and shader-translation rates.
/// No GPU/render pass — this is the parse+translate stage only.
fn scan_corpus(presets: &[PathBuf]) {
    let total = presets.len();
    let (mut loaded, mut load_fail) = (0usize, 0usize);
    let (mut shaders, mut shader_fail) = (0usize, 0usize);
    for p in presets {
        let Ok(bytes) = std::fs::read(p) else {
            load_fail += 1;
            continue;
        };
        let Ok(preset) = Preset::load(&String::from_utf8_lossy(&bytes)) else {
            load_fail += 1;
            continue;
        };
        loaded += 1;
        for (src, kind) in [
            (preset.warp_shader_source(), ShaderKind::Warp),
            (preset.composite_shader_source(), ShaderKind::Composite),
        ] {
            if src.contains("shader_body") {
                shaders += 1;
                if shader_to_wgsl(src, kind).is_err() {
                    shader_fail += 1;
                }
            }
        }
    }
    let pct = |n: usize, d: usize| if d == 0 { 100.0 } else { 100.0 * n as f64 / d as f64 };
    println!("── corpus compatibility scan ──");
    println!("  presets found:           {total}");
    println!("  loaded successfully:     {loaded}  ({:.1}%)", pct(loaded, total));
    println!("  skipped (load/parse):    {load_fail}");
    println!("  custom shaders:          {shaders}");
    println!(
        "  shader translate failed: {shader_fail}  ({:.1}% translate OK)",
        pct(shaders - shader_fail, shaders)
    );
    println!("  (translate stage only; naga validation + render not included)");
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_DIR.to_string());
    let mut presets = Vec::new();
    collect_presets(Path::new(&dir), &mut presets);
    if presets.is_empty() {
        eprintln!("No .milk presets found in {dir:?}; using the built-in preset.");
    } else {
        println!("Found {} presets in {dir}", presets.len());
    }
    if std::env::var_os("PM_SCAN").is_some() {
        scan_corpus(&presets);
    }
    // Load persisted preferences before finalizing startup state. `PM_PERF`
    // overrides the saved perf pref (forces on); `PM_SCAN` is a one-off, never
    // persisted.
    let prefs_path = prefs::config_path();
    let prefs = Prefs::load(&prefs_path);
    println!(
        "Keys: Right/Space/N next · Left/P prev · R random · F5/L reload · T transitions · F perf · H hud · Pause/K freeze (. step) · A auto ([ ] interval) · S shuffle · Esc/Q quit"
    );
    println!("Prefs: {}", prefs_path.display());

    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(presets, prefs, prefs_path);
    event_loop.run_app(&mut app).expect("run app");
}
