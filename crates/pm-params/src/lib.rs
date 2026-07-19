//! Reusable, platform-neutral control model for the visualizer: a controlled
//! visual clock, a tempo/beat system (auto/manual/tap), a small LFO bank, and a
//! parameter-modulation model. All pure logic, unit-tested on native, with no
//! GPU or browser dependency — so Phase 6 layers/effects can reuse it unchanged.
//!
//! Conventions: time is in seconds, `dt` is the per-frame step (the visualizer
//! uses a fixed 1/60 s, which keeps everything immune to tab-suspension delta
//! spikes). Audio "band" values (`bass`/`mid`/`treb`/`vol`) are projectM's
//! relative-loudness values (revolving around ~1.0), NOT browser `AnalyserNode`.

mod clock;
mod lfo;
mod param;
mod tempo;

pub use clock::VisualClock;
pub use lfo::{Lfo, LfoWave};
pub use param::{Curve, ModContext, ModSource, Parameter};
pub use tempo::{TapTempo, Tempo};

/// Per-frame audio analysis snapshot fed to the control model.
#[derive(Debug, Clone, Copy, Default)]
pub struct AudioFeatures {
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    pub vol: f32,
    pub bass_att: f32,
    pub mid_att: f32,
    pub treb_att: f32,
    pub vol_att: f32,
}
