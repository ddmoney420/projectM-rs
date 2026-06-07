//! Port of `Audio/WaveformAligner.{hpp,cpp}` — mip-based waveform alignment.
//!
//! Each frame's waveform is shifted to best-match the previous frame so visual
//! features stay put instead of jittering. It builds a mip pyramid (octaves),
//! then does a weighted absolute-error cross-correlation, refining the offset
//! search from coarsest to finest octave.

use crate::constants::{WaveformBuffer, AUDIO_BUFFER_SAMPLES, WAVEFORM_SAMPLES};

pub struct WaveformAligner {
    align_wave_ready: bool,
    alignment_weights: Vec<[f32; AUDIO_BUFFER_SAMPLES]>,
    octaves: usize,
    octave_samples: Vec<usize>,
    octave_sample_spacing: Vec<usize>,
    old_waveform_mips: Vec<WaveformBuffer>,
    first_nonzero_weights: Vec<usize>,
    last_nonzero_weights: Vec<usize>,
}

impl Default for WaveformAligner {
    fn default() -> Self {
        Self::new()
    }
}

impl WaveformAligner {
    pub fn new() -> Self {
        const MAX_OCTAVES: usize = 10;
        // floor(log2(AUDIO_BUFFER_SAMPLES - WAVEFORM_SAMPLES)); = floor(log2(96)) = 6.
        let num_octaves =
            ((AUDIO_BUFFER_SAMPLES - WAVEFORM_SAMPLES) as f32).ln() / 2.0f32.ln();
        let octaves = (num_octaves.floor() as usize).min(MAX_OCTAVES);

        let mut octave_samples = vec![0usize; octaves];
        let mut octave_sample_spacing = vec![0usize; octaves];
        octave_samples[0] = AUDIO_BUFFER_SAMPLES;
        octave_sample_spacing[0] = AUDIO_BUFFER_SAMPLES - WAVEFORM_SAMPLES;
        for octave in 1..octaves {
            octave_samples[octave] = octave_samples[octave - 1] / 2;
            octave_sample_spacing[octave] = octave_sample_spacing[octave - 1] / 2;
        }

        WaveformAligner {
            align_wave_ready: false,
            alignment_weights: vec![[0.0; AUDIO_BUFFER_SAMPLES]; octaves],
            octaves,
            octave_samples,
            octave_sample_spacing,
            old_waveform_mips: vec![[0.0; AUDIO_BUFFER_SAMPLES]; octaves],
            first_nonzero_weights: vec![0; octaves],
            last_nonzero_weights: vec![0; octaves],
        }
    }

    /// Aligns `new_waveform` in place to best-match the previous frame.
    pub fn align(&mut self, new_waveform: &mut WaveformBuffer) {
        if self.octaves < 4 {
            // Not enough margin to align; original milkdrop behavior.
            return;
        }

        let mut new_mips = vec![[0.0f32; AUDIO_BUFFER_SAMPLES]; self.octaves];
        Self::resample_octaves(&self.octave_samples, &mut new_mips, new_waveform);

        if !self.align_wave_ready {
            self.generate_weights();
            self.align_wave_ready = true;
        }

        let align_offset = self.calculate_offset(&new_mips);

        // Scoot aligned samples so they start at index 0.
        if align_offset > 0 {
            let off = align_offset as usize;
            new_waveform.copy_within(off..off + WAVEFORM_SAMPLES, 0);
            for s in new_waveform.iter_mut().take(AUDIO_BUFFER_SAMPLES).skip(WAVEFORM_SAMPLES) {
                *s = 0.0;
            }
        }

        // Recompute mips from the *shifted* waveform for next frame's reference.
        Self::resample_octaves(&self.octave_samples, &mut self.old_waveform_mips, new_waveform);
    }

    /// Octave 0 is a copy; each higher octave halves the previous via averaging.
    fn resample_octaves(
        octave_samples: &[usize],
        dst: &mut [WaveformBuffer],
        new_waveform: &WaveformBuffer,
    ) {
        dst[0].copy_from_slice(new_waveform);

        for octave in 1..octave_samples.len() {
            let (lo, hi) = dst.split_at_mut(octave);
            let prev = &lo[octave - 1];
            let cur = &mut hi[0];
            for sample in 0..octave_samples[octave] {
                cur[sample] = 0.5 * (prev[sample * 2] + prev[sample * 2 + 1]);
            }
        }
    }

    /// Builds the pyramid-shaped, center-emphasized correlation weights.
    /// Computed once on the first fill.
    fn generate_weights(&mut self) {
        for octave in 0..self.octaves {
            let compare_samples = self.octave_samples[octave] - self.octave_sample_spacing[octave];
            let weights = &mut self.alignment_weights[octave];

            // `sample` drives the pyramid arithmetic, so an index loop is clearest.
            #[allow(clippy::needless_range_loop)]
            for sample in 0..compare_samples {
                // Pyramid PDF 0..1..0.
                let mut w = if sample < compare_samples / 2 {
                    (sample * 2) as f32 / compare_samples as f32
                } else {
                    ((compare_samples - 1 - sample) * 2) as f32 / compare_samples as f32
                };

                // Emphasize the center vs. the edges, then clamp to [0, 1].
                w = (w - 0.8) * 5.0 + 0.8;
                w = w.clamp(0.0, 1.0);
                weights[sample] = w;
            }

            // First/last nonzero weight indices (the tweak zeroes ~64% of them).
            let mut sample = 0usize;
            while sample < compare_samples && weights[sample] == 0.0 {
                sample += 1;
            }
            self.first_nonzero_weights[octave] = sample;

            let mut s = compare_samples as isize - 1;
            while compare_samples > 1 && s >= 0 && weights[s as usize] == 0.0 {
                s -= 1;
            }
            self.last_nonzero_weights[octave] = s.max(0) as usize;
        }
    }

    /// Finds the shift (in samples) that minimizes weighted error vs. last frame.
    fn calculate_offset(&self, new_mips: &[WaveformBuffer]) -> i32 {
        let mut align_offset = 0i32;
        let mut offset_start = 0i32;
        let mut offset_end = self.octave_sample_spacing[self.octaves - 1] as i32;

        for octave in (0..self.octaves).rev() {
            let mut lowest_error_offset = -1i32;
            let mut lowest_error_amount = 0f32;

            for sample in offset_start..offset_end {
                let mut error_sum = 0f32;
                for i in self.first_nonzero_weights[octave]..=self.last_nonzero_weights[octave] {
                    let shifted = new_mips[octave][i + sample as usize];
                    let reference = self.old_waveform_mips[octave][i];
                    error_sum += ((shifted - reference) * self.alignment_weights[octave][i]).abs();
                }

                if lowest_error_offset == -1 || error_sum < lowest_error_amount {
                    lowest_error_offset = sample;
                    lowest_error_amount = error_sum;
                }
            }

            if octave > 0 {
                // Refine the search window around the best offset in the next octave.
                offset_start = lowest_error_offset * 2 - 1;
                offset_end = lowest_error_offset * 2 + 2 + 1;
                if offset_start < 0 {
                    offset_start = 0;
                }
                let limit = self.octave_sample_spacing[octave - 1] as i32;
                if offset_end > limit {
                    offset_end = limit;
                }
            } else {
                align_offset = lowest_error_offset;
            }
        }

        align_offset
    }
}
