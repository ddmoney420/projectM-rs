//! `pm-audio` — a Rust port of projectM's `Audio/` subsystem.
//!
//! It turns raw PCM into the per-frame data presets read: aligned left/right
//! **waveforms**, left/right **spectra**, and relative **bass/mid/treb** beat
//! values (the `bass`, `mid`, `treb` and `*_att` preset variables).
//!
//! # Pipeline
//!
//! ```text
//! add_float/add_i16/add_u8  →  circular input buffer
//!         update_frame_audio_data(dt, frame):
//!             copy → MilkdropFFT (spectrum) → WaveformAligner → Loudness (beat)
//!         frame_audio_data()  →  FrameAudioData
//! ```
//!
//! # Example
//!
//! ```
//! use pm_audio::PCM;
//!
//! let mut pcm = PCM::new();
//! // Feed one buffer of interleaved stereo f32 (here: silence).
//! let buf = vec![0.0f32; 1152]; // 576 frames * 2 channels
//! pcm.add_float(&buf, 2);
//! pcm.update_frame_audio_data(1.0 / 60.0, 0);
//!
//! let data = pcm.frame_audio_data();
//! assert_eq!(data.waveform_left.len(), pm_audio::WAVEFORM_SAMPLES);
//! ```

mod aligner;
mod constants;
mod fft;
mod loudness;
mod pcm;

pub use aligner::WaveformAligner;
pub use constants::{
    SpectrumBuffer, WaveformBuffer, AUDIO_BUFFER_SAMPLES, SPECTRUM_SAMPLES, WAVEFORM_SAMPLES,
};
pub use fft::MilkdropFFT;
pub use loudness::{Band, Loudness};
pub use pcm::{FrameAudioData, PCM};
