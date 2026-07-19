//! Tempo/beat model: automatic BPM (derived from projectM's bass loudness — not
//! `AnalyserNode`), manual override, and tap tempo, plus a beat clock exposing
//! `beat_phase`/`beat_pulse`/`beat_index`/`bar_phase`/confidence.
//!
//! Auto-detection is intentionally simple and stable: detect bass onsets, take a
//! robust average of recent inter-onset intervals, fold octaves into a musical
//! range, and smooth. It is approximate by design — manual/tap always overrides.

use crate::AudioFeatures;

const BAR_BEATS: u32 = 4;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;

/// Tap-tempo estimator: robust average of recent tap intervals.
#[derive(Debug, Clone, Default)]
pub struct TapTempo {
    taps: Vec<f64>,
}

impl TapTempo {
    /// Register a tap at wall-clock time `t` (seconds). Returns a BPM once there
    /// are enough taps. Gaps > 2 s start a fresh sequence.
    pub fn tap(&mut self, t: f64) -> Option<f32> {
        if let Some(&last) = self.taps.last() {
            if t - last > 2.0 {
                self.taps.clear();
            }
        }
        self.taps.push(t);
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        if self.taps.len() < 2 {
            return None;
        }
        let intervals: Vec<f64> = self.taps.windows(2).map(|w| w[1] - w[0]).collect();
        let mut sorted = intervals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        // Average intervals within 40% of the median (reject outliers).
        let good: Vec<f64> = intervals.iter().copied().filter(|&i| (i - median).abs() < median * 0.4).collect();
        let avg = good.iter().sum::<f64>() / good.len().max(1) as f64;
        if avg > 0.05 {
            Some((60.0 / avg) as f32)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.taps.clear();
    }
}

/// Bass-onset BPM detector.
#[derive(Debug, Clone)]
struct BeatDetector {
    avg: f32,
    since_onset: f32,
    armed: bool,
    intervals: Vec<f32>,
}

impl Default for BeatDetector {
    fn default() -> Self {
        BeatDetector { avg: 1.0, since_onset: 0.0, armed: true, intervals: Vec::new() }
    }
}

impl BeatDetector {
    /// Feed one frame. Returns `(bpm, confidence)` when a stable estimate exists.
    fn update(&mut self, dt: f32, audio: &AudioFeatures) -> Option<(f32, f32)> {
        let bass = audio.bass;
        // Slow-follow average of bass loudness.
        self.avg += (bass - self.avg) * 0.02;
        self.since_onset += dt;

        let threshold = self.avg * 1.3;
        if self.armed && bass > threshold && self.since_onset > 0.25 {
            // Onset: plausible tempo interval?
            if (0.33..=0.86).contains(&self.since_onset) {
                self.intervals.push(self.since_onset);
                if self.intervals.len() > 8 {
                    self.intervals.remove(0);
                }
            }
            self.since_onset = 0.0;
            self.armed = false;
        }
        if bass < self.avg * 1.05 {
            self.armed = true; // re-arm once bass dips
        }

        if self.intervals.len() < 4 {
            return None;
        }
        let mut sorted = self.intervals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = sorted[sorted.len() / 2];
        let good = self.intervals.iter().filter(|&&i| (i - median).abs() < median * 0.15).count();
        let confidence = good as f32 / self.intervals.len() as f32;
        Some((fold_bpm(60.0 / median), confidence))
    }
}

/// Fold a BPM into `[MIN_BPM, MAX_BPM)` by octave doubling/halving.
fn fold_bpm(mut bpm: f32) -> f32 {
    while bpm < MIN_BPM {
        bpm *= 2.0;
    }
    while bpm >= MAX_BPM {
        bpm *= 0.5;
    }
    bpm
}

/// The tempo/beat clock.
#[derive(Debug, Clone)]
pub struct Tempo {
    bpm: f32,
    manual: bool,
    subdivision: f32,
    beat_phase: f32,
    beat_index: u64,
    beat_pulse: f32,
    confidence: f32,
    detector: BeatDetector,
    tap: TapTempo,
}

impl Default for Tempo {
    fn default() -> Self {
        Tempo {
            bpm: 120.0,
            manual: false,
            subdivision: 1.0,
            beat_phase: 0.0,
            beat_index: 0,
            beat_pulse: 0.0,
            confidence: 0.0,
            detector: BeatDetector::default(),
            tap: TapTempo::default(),
        }
    }
}

impl Tempo {
    /// Advance the beat clock by `dt` (visual seconds) and, in auto mode, refine
    /// the BPM from `audio`.
    pub fn update(&mut self, dt: f32, audio: &AudioFeatures) {
        if !self.manual {
            if let Some((bpm, conf)) = self.detector.update(dt, audio) {
                self.bpm += (bpm - self.bpm) * 0.08; // smooth toward estimate
                self.confidence = conf;
            }
        }
        let beats_per_sec = (self.bpm / 60.0) * self.subdivision;
        self.beat_phase += dt * beats_per_sec;
        while self.beat_phase >= 1.0 {
            self.beat_phase -= 1.0;
            self.beat_index += 1;
        }
        // Short transient decaying over the first quarter of the beat.
        self.beat_pulse = (1.0 - self.beat_phase * 4.0).max(0.0);
    }

    /// Register a tap (wall-clock seconds); switches to manual and resets phase.
    pub fn tap(&mut self, t: f64) {
        if let Some(bpm) = self.tap.tap(t) {
            self.bpm = fold_bpm(bpm);
            self.manual = true;
            self.confidence = 1.0;
        }
        self.beat_phase = 0.0;
    }

    pub fn set_manual_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.clamp(20.0, 400.0);
        self.manual = true;
        self.confidence = 1.0;
    }
    pub fn set_manual(&mut self, manual: bool) {
        self.manual = manual;
    }
    pub fn set_subdivision(&mut self, sub: f32) {
        self.subdivision = sub.max(0.0625);
    }
    pub fn half_time(&mut self) {
        self.bpm = (self.bpm * 0.5).max(20.0);
        self.manual = true;
    }
    pub fn double_time(&mut self) {
        self.bpm = (self.bpm * 2.0).min(400.0);
        self.manual = true;
    }
    pub fn reset_phase(&mut self) {
        self.beat_phase = 0.0;
        self.beat_index = 0;
    }

    pub fn bpm(&self) -> f32 {
        self.bpm
    }
    pub fn manual(&self) -> bool {
        self.manual
    }
    pub fn beat_phase(&self) -> f32 {
        self.beat_phase
    }
    pub fn beat_pulse(&self) -> f32 {
        self.beat_pulse
    }
    pub fn beat_index(&self) -> u64 {
        self.beat_index
    }
    pub fn confidence(&self) -> f32 {
        self.confidence
    }
    pub fn bar_phase(&self) -> f32 {
        ((self.beat_index % BAR_BEATS as u64) as f32 + self.beat_phase) / BAR_BEATS as f32
    }
    /// Continuous beat position (integer + fractional beats) for LFO tempo sync.
    pub fn beat_time(&self) -> f32 {
        self.beat_index as f32 + self.beat_phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_bpm_sets_phase_rate() {
        let mut t = Tempo::default();
        t.set_manual_bpm(120.0); // 2 beats/sec
        let audio = AudioFeatures::default();
        // 0.25 s → half a beat.
        for _ in 0..15 {
            t.update(1.0 / 60.0, &audio);
        }
        assert!((t.beat_phase() - 0.5).abs() < 0.05, "phase {}", t.beat_phase());
    }

    #[test]
    fn beat_index_increments() {
        let mut t = Tempo::default();
        t.set_manual_bpm(120.0);
        let audio = AudioFeatures::default();
        for _ in 0..60 {
            t.update(1.0 / 60.0, &audio); // 1 s → 2 beats
        }
        assert_eq!(t.beat_index(), 2);
    }

    #[test]
    fn half_and_double() {
        let mut t = Tempo::default();
        t.set_manual_bpm(120.0);
        t.half_time();
        assert_eq!(t.bpm(), 60.0);
        t.double_time();
        t.double_time();
        assert_eq!(t.bpm(), 240.0);
    }

    #[test]
    fn tap_tempo_from_intervals() {
        let mut tap = TapTempo::default();
        // Taps every 0.5 s → 120 BPM.
        assert_eq!(tap.tap(0.0), None);
        let mut bpm = 0.0;
        for i in 1..=5 {
            if let Some(b) = tap.tap(i as f64 * 0.5) {
                bpm = b;
            }
        }
        assert!((bpm - 120.0).abs() < 1.0, "bpm {bpm}");
    }

    #[test]
    fn tap_gap_resets_sequence() {
        let mut tap = TapTempo::default();
        tap.tap(0.0);
        tap.tap(0.5);
        // Long gap → new sequence, not a huge interval.
        assert_eq!(tap.tap(100.0), None);
    }

    #[test]
    fn auto_detects_120_from_onsets() {
        let mut t = Tempo::default();
        // Synthesize bass onsets every 0.5 s (120 BPM) for ~6 s.
        for frame in 0..360 {
            let time = frame as f32 / 60.0;
            let beat = (time * 2.0).fract(); // 2 beats/sec
            let bass = if beat < 0.1 { 2.5 } else { 0.8 };
            let audio = AudioFeatures { bass, ..Default::default() };
            t.update(1.0 / 60.0, &audio);
        }
        assert!(t.confidence() > 0.5, "confidence {}", t.confidence());
        assert!((t.bpm() - 120.0).abs() < 12.0, "bpm {}", t.bpm());
    }

    #[test]
    fn bar_phase_spans_four_beats() {
        let mut t = Tempo::default();
        t.set_manual_bpm(120.0);
        let audio = AudioFeatures::default();
        for _ in 0..60 {
            t.update(1.0 / 60.0, &audio); // 2 beats → bar_phase ~0.5
        }
        assert!((t.bar_phase() - 0.5).abs() < 0.05, "bar {}", t.bar_phase());
    }
}
