//! Preset transition (crossfade) tests for `PresetPlayer`.

use pm_audio::FrameAudioData;
use pm_core::{transition_progress, PresetPlayer, WarpEngine, DEFAULT_TRANSITION_SECS};
use pm_preset::Preset;
use pm_render::GpuContext;

fn engine(ctx: &GpuContext, src: &str) -> WarpEngine {
    WarpEngine::new(ctx, Preset::load(src).unwrap(), 64, 64)
}

#[test]
fn transition_progress_is_time_based_and_clamped() {
    // Linear ramp over the window, clamped to [0, 1].
    assert_eq!(transition_progress(10.0, 2.0, 10.0), 0.0);
    assert_eq!(transition_progress(10.0, 2.0, 11.0), 0.5);
    assert_eq!(transition_progress(10.0, 2.0, 12.0), 1.0);
    assert_eq!(transition_progress(10.0, 2.0, 13.0), 1.0); // clamped past the end
    assert_eq!(transition_progress(10.0, 2.0, 9.0), 0.0); // clamped before the start
    // Zero (or negative) duration is an instant cut.
    assert_eq!(transition_progress(10.0, 0.0, 10.0), 1.0);
    // The default is Milkdrop's soft-cut length.
    assert_eq!(transition_progress(0.0, DEFAULT_TRANSITION_SECS, DEFAULT_TRANSITION_SECS), 1.0);
}

#[test]
fn zero_duration_is_an_immediate_hard_cut() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut player = PresetPlayer::new(&ctx, engine(&ctx, "zoom=1.01"), 64, 64, 0.0);
    player.render(&ctx, 0.0, FrameAudioData::default());

    player.switch_to(engine(&ctx, "zoom=1.02"));
    assert!(!player.is_transitioning(), "duration 0 should not start a transition");
    assert_eq!(player.progress(), 0.0);
}

#[test]
fn transition_runs_then_cleans_up_outgoing() {
    let Ok(ctx) = GpuContext::headless() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut player = PresetPlayer::new(&ctx, engine(&ctx, "zoom=1.01\nnWaveMode=0"), 64, 64, 2.0);

    // Establish a current time so the transition starts at t = 1.0.
    player.render(&ctx, 1.0, FrameAudioData::default());

    // Switch: a crossfade begins.
    player.switch_to(engine(&ctx, "zoom=1.03\nnWaveMode=6"));
    assert!(player.is_transitioning(), "a positive duration should start a transition");

    // Midway through the 2s window: progress ~0.5, still transitioning, blended
    // output is used.
    player.render(&ctx, 2.0, FrameAudioData::default());
    assert!(player.is_transitioning());
    let mid = player.progress();
    assert!((0.4..=0.6).contains(&mid), "midpoint progress ~0.5, got {mid}");

    // Past the end of the window: the transition completes and the outgoing
    // preset is dropped.
    player.render(&ctx, 3.5, FrameAudioData::default());
    assert!(!player.is_transitioning(), "transition should be finished and cleaned up");
    assert_eq!(player.progress(), 0.0);
}
