//! Low-frequency oscillators for modulation. One implementation, usable both
//! free-running (driven by the visual clock) and tempo-synchronized (phase from
//! the beat clock). Output is unipolar `[0, 1]` for direct use as a modulation
//! amount.

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWave {
    Sine,
    Triangle,
    Saw,
    Square,
}

impl LfoWave {
    /// Evaluate the waveform at phase `p` in `[0, 1)`, returned unipolar `[0,1]`.
    pub fn eval(self, p: f32) -> f32 {
        let p = p.rem_euclid(1.0);
        match self {
            LfoWave::Sine => 0.5 - 0.5 * (2.0 * PI * p).cos(), // starts at 0, peak at 0.5
            LfoWave::Triangle => 1.0 - (2.0 * p - 1.0).abs(),
            LfoWave::Saw => p,
            LfoWave::Square => {
                if p < 0.5 {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }
}

/// A single LFO. Free-running mode advances its own phase from the visual clock
/// delta at `rate` Hz; tempo-sync mode reads phase from the beat clock scaled by
/// `mult` (beats per cycle).
#[derive(Debug, Clone)]
pub struct Lfo {
    pub wave: LfoWave,
    pub rate_hz: f32,
    pub phase_offset: f32,
    pub tempo_sync: bool,
    /// Cycles per beat when tempo-synced (e.g. 0.25 = one cycle per 4 beats).
    pub mult: f32,
    free_phase: f32,
    value: f32,
}

impl Default for Lfo {
    fn default() -> Self {
        Lfo {
            wave: LfoWave::Sine,
            rate_hz: 1.0,
            phase_offset: 0.0,
            tempo_sync: false,
            mult: 1.0,
            free_phase: 0.0,
            value: 0.0,
        }
    }
}

impl Lfo {
    /// Advance and evaluate. `dt` is the visual delta; `beat_time` is a
    /// continuous beat position (integer + fractional beats) for tempo sync.
    pub fn update(&mut self, dt: f32, beat_time: f32) -> f32 {
        let phase = if self.tempo_sync {
            beat_time * self.mult + self.phase_offset
        } else {
            self.free_phase += dt * self.rate_hz;
            self.free_phase + self.phase_offset
        };
        self.value = self.wave.eval(phase);
        self.value
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_stay_in_unit_range() {
        for wave in [LfoWave::Sine, LfoWave::Triangle, LfoWave::Saw, LfoWave::Square] {
            for i in 0..1000 {
                let v = wave.eval(i as f32 / 250.0);
                assert!((0.0..=1.0).contains(&v), "{wave:?} out of range: {v}");
            }
        }
    }

    #[test]
    fn saw_is_linear_ramp() {
        assert!((LfoWave::Saw.eval(0.0)).abs() < 1e-6);
        assert!((LfoWave::Saw.eval(0.5) - 0.5).abs() < 1e-6);
        assert!((LfoWave::Saw.eval(0.999) - 0.999).abs() < 1e-3);
    }

    #[test]
    fn square_halves() {
        assert_eq!(LfoWave::Square.eval(0.25), 0.0);
        assert_eq!(LfoWave::Square.eval(0.75), 1.0);
    }

    #[test]
    fn free_running_completes_cycle_at_rate() {
        let mut lfo = Lfo { wave: LfoWave::Saw, rate_hz: 1.0, ..Default::default() };
        // 1 Hz saw over 1 s (60 frames of 1/60) returns near a full cycle.
        let mut last = 0.0;
        for _ in 0..59 {
            last = lfo.update(1.0 / 60.0, 0.0);
        }
        assert!(last > 0.95, "expected near end of ramp, got {last}");
    }

    #[test]
    fn tempo_sync_follows_beat_time() {
        let mut lfo = Lfo { wave: LfoWave::Saw, tempo_sync: true, mult: 1.0, ..Default::default() };
        assert!((lfo.update(0.0, 0.0)).abs() < 1e-6);
        assert!((lfo.update(0.0, 0.5) - 0.5).abs() < 1e-6);
    }
}
