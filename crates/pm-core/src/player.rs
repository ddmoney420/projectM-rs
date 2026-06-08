//! [`PresetPlayer`] — drives one preset and smoothly crossfades to the next.
//!
//! Switching presets mid-playback hard-cuts by default; with a transition
//! duration set (Milkdrop's default is ~2.7s), the outgoing preset is kept alive
//! and keeps evolving while the incoming one develops, and the two are
//! crossfaded over an elapsed-time window. The outgoing preset is dropped as
//! soon as the blend completes.

use crate::crossfade::Crossfade;
use crate::WarpEngine;
use pm_audio::FrameAudioData;
use pm_render::{GpuContext, Texture, TARGET_FORMAT};

/// Milkdrop's default soft-cut transition length, in seconds.
pub const DEFAULT_TRANSITION_SECS: f32 = 2.7;

/// Blend factor in `0..=1` for a transition that started at `start`, given the
/// current `time` and `duration`. `duration <= 0` is an instant cut (`1.0`).
pub fn transition_progress(start: f32, duration: f32, time: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    ((time - start) / duration).clamp(0.0, 1.0)
}

struct Transition {
    /// Elapsed-time stamp when the transition began.
    start: f32,
}

pub struct PresetPlayer {
    current: WarpEngine,
    outgoing: Option<WarpEngine>,
    transition: Option<Transition>,
    blend: Crossfade,
    /// Blend target while transitioning.
    output: Texture,
    /// Transition length in seconds; `0` means hard cut.
    duration: f32,
    current_frame: i32,
    outgoing_frame: i32,
    /// Last `time` seen by [`Self::render`], used as a transition's start stamp.
    last_time: f32,
}

impl PresetPlayer {
    /// Wrap an initial engine. `duration` is the crossfade length in seconds
    /// (0 = hard cut). The engine and output are sized `width`×`height`.
    pub fn new(ctx: &GpuContext, engine: WarpEngine, width: u32, height: u32, duration: f32) -> Self {
        PresetPlayer {
            current: engine,
            outgoing: None,
            transition: None,
            blend: Crossfade::new(ctx),
            output: Texture::new_render_target(&ctx.device, "preset player output", width, height, TARGET_FORMAT),
            duration: duration.max(0.0),
            current_frame: 0,
            outgoing_frame: 0,
            last_time: 0.0,
        }
    }

    /// Set the transition length (seconds). `0` disables crossfades.
    pub fn set_duration(&mut self, secs: f32) {
        self.duration = secs.max(0.0);
    }

    pub fn duration(&self) -> f32 {
        self.duration
    }

    /// Switch to `engine`. With a positive duration this starts a crossfade from
    /// the current preset; otherwise it cuts immediately.
    pub fn switch_to(&mut self, engine: WarpEngine) {
        if self.duration <= 0.0 {
            self.current = engine;
            self.current_frame = 0;
            self.outgoing = None;
            self.transition = None;
            return;
        }
        // The current preset becomes the outgoing one; both keep rendering.
        let old = std::mem::replace(&mut self.current, engine);
        self.outgoing = Some(old);
        self.outgoing_frame = self.current_frame;
        self.current_frame = 0;
        self.transition = Some(Transition { start: self.last_time });
    }

    /// Whether a crossfade is currently in progress.
    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some()
    }

    /// Current blend factor (`0` when not transitioning).
    pub fn progress(&self) -> f32 {
        match &self.transition {
            Some(t) => transition_progress(t.start, self.duration, self.last_time),
            None => 0.0,
        }
    }

    /// The engine being faded in (the visible/most-recent preset).
    pub fn current_engine(&self) -> &WarpEngine {
        &self.current
    }

    /// Advance the preset(s) for this frame at absolute `time` (seconds), then
    /// (when transitioning) blend the outgoing and incoming outputs.
    pub fn render(&mut self, ctx: &GpuContext, time: f32, audio: FrameAudioData) {
        self.last_time = time;

        // The incoming/active preset always advances.
        let _ = self.current.render_frame(ctx, time, self.current_frame, audio.clone());
        self.current_frame += 1;

        let Some(t) = &self.transition else { return };

        // The outgoing preset keeps evolving independently during the fade.
        if let Some(out) = &mut self.outgoing {
            let _ = out.render_frame(ctx, time, self.outgoing_frame, audio);
            self.outgoing_frame += 1;
        }

        let progress = transition_progress(t.start, self.duration, time);
        if let Some(out) = &self.outgoing {
            self.blend.draw(ctx, out.display_texture(), self.current.display_texture(), progress, &self.output);
        }

        // Done: drop the outgoing preset cleanly.
        if progress >= 1.0 {
            self.outgoing = None;
            self.transition = None;
        }
    }

    /// The texture to present: the blended output mid-transition, else the
    /// current preset's display texture directly.
    pub fn output_texture(&self) -> &Texture {
        if self.transition.is_some() {
            &self.output
        } else {
            self.current.display_texture()
        }
    }
}
