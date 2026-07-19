//! The parameter-modulation model. A [`Parameter`] has a base value plus one
//! modulation source, scaled by amount, shaped by a response curve, smoothed,
//! and clamped. The same model drives shader controls now and (unchanged)
//! layer/effect/transition parameters in Phase 6.

use crate::AudioFeatures;

/// What modulates a parameter. `Lfo(i)` indexes the shared LFO bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSource {
    None,
    Bass,
    Mid,
    Treb,
    Vol,
    BassAtt,
    MidAtt,
    TrebAtt,
    VolAtt,
    BeatPulse,
    BeatPhase,
    Lfo(u8),
}

impl ModSource {
    /// Parse the UI/string name (e.g. from persisted state or a dropdown).
    pub fn from_str(s: &str) -> ModSource {
        match s {
            "bass" => ModSource::Bass,
            "mid" => ModSource::Mid,
            "treb" => ModSource::Treb,
            "vol" => ModSource::Vol,
            "bassAtt" => ModSource::BassAtt,
            "midAtt" => ModSource::MidAtt,
            "trebAtt" => ModSource::TrebAtt,
            "volAtt" => ModSource::VolAtt,
            "beatPulse" => ModSource::BeatPulse,
            "beatPhase" => ModSource::BeatPhase,
            "lfo0" => ModSource::Lfo(0),
            "lfo1" => ModSource::Lfo(1),
            "lfo2" => ModSource::Lfo(2),
            "lfo3" => ModSource::Lfo(3),
            _ => ModSource::None,
        }
    }
}

/// Response shape applied to the (unit) modulation input before scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Linear,
    Exp,
    Log,
    SCurve,
}

impl Curve {
    pub fn apply(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Linear => x,
            Curve::Exp => x * x,
            Curve::Log => x.sqrt(),
            Curve::SCurve => x * x * (3.0 - 2.0 * x),
        }
    }
}

/// The current modulation inputs for a frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModContext {
    pub audio: AudioFeatures,
    pub beat_phase: f32,
    pub beat_pulse: f32,
    pub lfo: [f32; 4],
}

impl ModContext {
    /// Sample a source as a (roughly) unit modulation signal. Bands return the
    /// reactive deviation above their ~1.0 baseline, so a resting signal
    /// contributes 0 and hits push positive.
    pub fn sample(&self, source: ModSource) -> f32 {
        let band = |v: f32| (v - 1.0).clamp(0.0, 2.0);
        match source {
            ModSource::None => 0.0,
            ModSource::Bass => band(self.audio.bass),
            ModSource::Mid => band(self.audio.mid),
            ModSource::Treb => band(self.audio.treb),
            ModSource::Vol => band(self.audio.vol),
            ModSource::BassAtt => band(self.audio.bass_att),
            ModSource::MidAtt => band(self.audio.mid_att),
            ModSource::TrebAtt => band(self.audio.treb_att),
            ModSource::VolAtt => band(self.audio.vol_att),
            ModSource::BeatPulse => self.beat_pulse.clamp(0.0, 1.0),
            ModSource::BeatPhase => self.beat_phase.clamp(0.0, 1.0),
            ModSource::Lfo(i) => self.lfo.get(i as usize).copied().unwrap_or(0.0),
        }
    }
}

/// A modulatable parameter with smoothing and clamping.
#[derive(Debug, Clone)]
pub struct Parameter {
    pub base: f32,
    pub source: ModSource,
    pub amount: f32,
    /// One-pole smoothing in `[0, 1)`; 0 = instant, higher = slower.
    pub smoothing: f32,
    pub min: f32,
    pub max: f32,
    pub invert: bool,
    pub curve: Curve,
    smoothed: f32,
    primed: bool,
}

impl Parameter {
    pub fn new(base: f32, min: f32, max: f32) -> Self {
        Parameter {
            base,
            source: ModSource::None,
            amount: 0.0,
            smoothing: 0.0,
            min,
            max,
            invert: false,
            curve: Curve::Linear,
            smoothed: base,
            primed: false,
        }
    }

    /// Evaluate for this frame and return the smoothed, clamped value.
    pub fn eval(&mut self, ctx: &ModContext) -> f32 {
        let mut m = ctx.sample(self.source);
        if self.invert {
            m = 1.0 - m.clamp(0.0, 1.0);
        }
        m = self.curve.apply(m);
        let target = (self.base + m * self.amount).clamp(self.min, self.max);
        if !self.primed {
            self.smoothed = target;
            self.primed = true;
        } else {
            self.smoothed += (target - self.smoothed) * (1.0 - self.smoothing.clamp(0.0, 0.999));
        }
        self.smoothed
    }

    pub fn value(&self) -> f32 {
        self.smoothed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_bass(bass: f32) -> ModContext {
        ModContext { audio: AudioFeatures { bass, ..Default::default() }, ..Default::default() }
    }

    #[test]
    fn base_only_returns_base() {
        let mut p = Parameter::new(0.5, 0.0, 1.0);
        assert_eq!(p.eval(&ModContext::default()), 0.5);
    }

    #[test]
    fn bass_modulation_raises_value() {
        let mut p = Parameter::new(0.2, 0.0, 1.0);
        p.source = ModSource::Bass;
        p.amount = 0.5;
        let resting = { let mut q = p.clone(); q.eval(&ctx_with_bass(1.0)) };
        let hit = p.eval(&ctx_with_bass(2.0)); // deviation 1.0 → +0.5
        assert!((resting - 0.2).abs() < 1e-6);
        assert!(hit > resting, "hit {hit} resting {resting}");
    }

    #[test]
    fn clamped_to_range() {
        let mut p = Parameter::new(0.9, 0.0, 1.0);
        p.source = ModSource::Bass;
        p.amount = 10.0;
        let v = p.eval(&ctx_with_bass(3.0));
        assert!(v <= 1.0);
    }

    #[test]
    fn smoothing_moves_gradually() {
        let mut p = Parameter::new(0.0, 0.0, 1.0);
        p.source = ModSource::Vol;
        p.amount = 1.0;
        p.smoothing = 0.9;
        p.eval(&ModContext::default()); // primes at 0
        let ctx = ModContext { audio: AudioFeatures { vol: 2.0, ..Default::default() }, ..Default::default() };
        let first = p.eval(&ctx);
        assert!(first < 0.5, "should not jump instantly: {first}");
    }

    #[test]
    fn invert_flips() {
        let mut p = Parameter::new(0.0, 0.0, 1.0);
        p.source = ModSource::BeatPhase;
        p.amount = 1.0;
        p.invert = true;
        let ctx = ModContext { beat_phase: 0.0, ..Default::default() };
        // inverted phase 0 → 1.0 → *amount 1.0.
        assert!((p.eval(&ctx) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn curve_shapes_input() {
        assert_eq!(Curve::Exp.apply(0.5), 0.25);
        assert!((Curve::Log.apply(0.25) - 0.5).abs() < 1e-6);
        assert_eq!(Curve::SCurve.apply(0.5), 0.5);
    }
}
