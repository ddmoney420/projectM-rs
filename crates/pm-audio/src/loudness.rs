//! Port of `Audio/Loudness.{hpp,cpp}` — per-band beat detection. Tracks a band
//! sum against short- and long-term averages to produce a loudness *relative*
//! to recent history (revolving around 1.0).

use crate::constants::{SpectrumBuffer, SPECTRUM_SAMPLES};

/// Frequency band. Only the lower half of the spectrum is used; each band takes
/// one sixth of the full spectrum (i.e. one third of the used half).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Bass = 0,
    Middles = 1,
    Treble = 2,
}

/// Beat-detection loudness for a single frequency band.
#[derive(Debug, Clone)]
pub struct Loudness {
    band: Band,
    current: f32,
    average: f32,
    long_average: f32,
    current_relative: f32,
    average_relative: f32,
}

impl Loudness {
    pub fn new(band: Band) -> Self {
        Loudness {
            band,
            current: 0.0,
            average: 0.0,
            long_average: 0.0,
            current_relative: 1.0,
            average_relative: 1.0,
        }
    }

    /// Update from this frame's spectrum. Call exactly once per frame.
    /// `frame` ramps the long-term smoothing for the first ~50 frames.
    pub fn update(&mut self, spectrum: &SpectrumBuffer, seconds_since_last_frame: f64, frame: u32) {
        self.sum_band(spectrum);
        self.update_band_average(seconds_since_last_frame, frame);
    }

    /// Current unattenuated loudness relative to recent history (~1.0 nominal).
    pub fn current_relative(&self) -> f32 {
        self.current_relative
    }

    /// Attenuated (smoothed) loudness relative to recent history (~1.0 nominal).
    pub fn average_relative(&self) -> f32 {
        self.average_relative
    }

    fn sum_band(&mut self, spectrum: &SpectrumBuffer) {
        let b = self.band as i32;
        let start = SPECTRUM_SAMPLES as i32 * b / 6;
        let end = SPECTRUM_SAMPLES as i32 * (b + 1) / 6;

        self.current = 0.0;
        for sample in start..end {
            self.current += spectrum[sample as usize];
        }
    }

    fn update_band_average(&mut self, seconds_since_last_frame: f64, frame: u32) {
        let mut rate = Self::adjust_rate_to_fps(
            if self.current > self.average { 0.2 } else { 0.5 },
            seconds_since_last_frame,
        );
        self.average = self.average * rate + self.current * (1.0 - rate);

        rate = Self::adjust_rate_to_fps(
            if frame < 50 { 0.9 } else { 0.992 },
            seconds_since_last_frame,
        );
        self.long_average = self.long_average * rate + self.current * (1.0 - rate);

        if self.long_average.abs() < 0.001 {
            self.current_relative = 1.0;
            self.average_relative = 1.0;
        } else {
            self.current_relative = self.current / self.long_average;
            self.average_relative = self.average / self.long_average;
        }
    }

    /// Re-scale a per-(1/30s) decay rate to the actual frame duration.
    fn adjust_rate_to_fps(rate: f32, seconds_since_last_frame: f64) -> f32 {
        let per_second = rate.powf(30.0);
        per_second.powf(seconds_since_last_frame as f32)
    }
}
