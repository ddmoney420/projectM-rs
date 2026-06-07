//! Audio buffer sizes, ported from `Audio/AudioConstants.hpp`.

/// Number of waveform samples stored in the analysis buffer.
pub const AUDIO_BUFFER_SAMPLES: usize = 576;
/// Number of waveform samples available for rendering a single frame.
pub const WAVEFORM_SAMPLES: usize = 480;
/// Number of spectrum-analyzer samples produced per channel.
pub const SPECTRUM_SAMPLES: usize = 512;

/// Buffer holding waveform data. Only the first [`WAVEFORM_SAMPLES`] are valid
/// for rendering; the full length is used for analysis/alignment.
pub type WaveformBuffer = [f32; AUDIO_BUFFER_SAMPLES];
/// Buffer holding spectrum data.
pub type SpectrumBuffer = [f32; SPECTRUM_SAMPLES];
