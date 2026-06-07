//! Port of `Audio/PCM.{hpp,cpp}` and `FrameAudioData.hpp` — the public entry
//! point. Accepts raw interleaved PCM, then on each frame produces aligned
//! waveforms, spectra, and bass/mid/treb beat values.

use crate::aligner::WaveformAligner;
use crate::constants::*;
use crate::fft::MilkdropFFT;
use crate::loudness::{Band, Loudness};

/// All audio data needed to render a single frame.
///
/// Only the first [`WAVEFORM_SAMPLES`] waveform values and [`SPECTRUM_SAMPLES`]
/// spectrum values are valid (the arrays are sized exactly to those).
#[derive(Debug, Clone)]
pub struct FrameAudioData {
    pub bass: f32,
    pub bass_att: f32,
    pub mid: f32,
    pub mid_att: f32,
    pub treb: f32,
    pub treb_att: f32,
    pub vol: f32,
    pub vol_att: f32,
    pub waveform_left: [f32; WAVEFORM_SAMPLES],
    pub waveform_right: [f32; WAVEFORM_SAMPLES],
    pub spectrum_left: [f32; SPECTRUM_SAMPLES],
    pub spectrum_right: [f32; SPECTRUM_SAMPLES],
}

impl Default for FrameAudioData {
    fn default() -> Self {
        FrameAudioData {
            bass: 0.0,
            bass_att: 0.0,
            mid: 0.0,
            mid_att: 0.0,
            treb: 0.0,
            treb_att: 0.0,
            vol: 0.0,
            vol_att: 0.0,
            waveform_left: [0.0; WAVEFORM_SAMPLES],
            waveform_right: [0.0; WAVEFORM_SAMPLES],
            spectrum_left: [0.0; SPECTRUM_SAMPLES],
            spectrum_right: [0.0; SPECTRUM_SAMPLES],
        }
    }
}

/// Audio analyzer: feed it PCM via the `add_*` methods (typically from an audio
/// thread), then call [`PCM::update_frame_audio_data`] once per rendered frame.
///
/// Unlike the C++ original this type holds no internal mutex; if you feed it
/// from a separate thread, synchronize access externally (e.g. wrap in a
/// `Mutex<PCM>`).
pub struct PCM {
    input_buffer_l: WaveformBuffer,
    input_buffer_r: WaveformBuffer,
    start: usize,

    waveform_l: WaveformBuffer,
    waveform_r: WaveformBuffer,

    spectrum_l: SpectrumBuffer,
    spectrum_r: SpectrumBuffer,

    fft: MilkdropFFT,
    align_l: WaveformAligner,
    align_r: WaveformAligner,

    bass: Loudness,
    middles: Loudness,
    treble: Loudness,
}

impl Default for PCM {
    fn default() -> Self {
        Self::new()
    }
}

impl PCM {
    pub fn new() -> Self {
        PCM {
            input_buffer_l: [0.0; AUDIO_BUFFER_SAMPLES],
            input_buffer_r: [0.0; AUDIO_BUFFER_SAMPLES],
            start: 0,
            waveform_l: [0.0; AUDIO_BUFFER_SAMPLES],
            waveform_r: [0.0; AUDIO_BUFFER_SAMPLES],
            spectrum_l: [0.0; SPECTRUM_SAMPLES],
            spectrum_r: [0.0; SPECTRUM_SAMPLES],
            fft: MilkdropFFT::new(WAVEFORM_SAMPLES, SPECTRUM_SAMPLES, true, 1.0),
            align_l: WaveformAligner::new(),
            align_r: WaveformAligner::new(),
            bass: Loudness::new(Band::Bass),
            middles: Loudness::new(Band::Middles),
            treble: Loudness::new(Band::Treble),
        }
    }

    /// Add interleaved 32-bit float PCM. Channel 0 is left, channel 1 right;
    /// any further channels are ignored. Mono is duplicated to both channels.
    pub fn add_float(&mut self, samples: &[f32], channels: u32) {
        self.add_to_buffer(samples, channels, |s| 128.0 * s);
    }

    /// Add interleaved unsigned 8-bit PCM (offset 128, amplitude 128).
    pub fn add_u8(&mut self, samples: &[u8], channels: u32) {
        self.add_to_buffer(samples, channels, |s| s as f32 - 128.0);
    }

    /// Add interleaved signed 16-bit PCM (amplitude 32768).
    pub fn add_i16(&mut self, samples: &[i16], channels: u32) {
        self.add_to_buffer(samples, channels, |s| s as f32 / 256.0);
    }

    fn add_to_buffer<T: Copy>(&mut self, samples: &[T], channels: u32, conv: impl Fn(T) -> f32) {
        if channels == 0 || samples.is_empty() {
            return;
        }
        let ch = channels as usize;
        let count = samples.len() / ch;
        for i in 0..count {
            let off = (self.start + i) % AUDIO_BUFFER_SAMPLES;
            let l = conv(samples[i * ch]);
            self.input_buffer_l[off] = l;
            self.input_buffer_r[off] = if ch > 1 { conv(samples[1 + i * ch]) } else { l };
        }
        self.start = (self.start + count) % AUDIO_BUFFER_SAMPLES;
    }

    /// Advance one frame: copy input, run the spectrum analyzer, align the
    /// waveforms, and update beat detection. Call exactly once per frame.
    pub fn update_frame_audio_data(&mut self, seconds_since_last_frame: f64, frame: u32) {
        // 1. Snapshot the circular input buffer into the per-frame waveforms.
        Self::copy_new_waveform_data(self.start, &self.input_buffer_l, &mut self.waveform_l);
        Self::copy_new_waveform_data(self.start, &self.input_buffer_r, &mut self.waveform_r);

        // 2. Spectrum analyzer for both channels.
        Self::update_spectrum(&self.fft, &self.waveform_l, &mut self.spectrum_l);
        Self::update_spectrum(&self.fft, &self.waveform_r, &mut self.spectrum_r);

        // 3. Align waveforms to the previous frame.
        self.align_l.align(&mut self.waveform_l);
        self.align_r.align(&mut self.waveform_r);

        // 4. Beat detection (all three bands use the left spectrum, as upstream).
        self.bass.update(&self.spectrum_l, seconds_since_last_frame, frame);
        self.middles.update(&self.spectrum_l, seconds_since_last_frame, frame);
        self.treble.update(&self.spectrum_l, seconds_since_last_frame, frame);
    }

    /// Snapshot of the current frame's analyzed audio data.
    pub fn frame_audio_data(&self) -> FrameAudioData {
        let mut data = FrameAudioData::default();

        data.waveform_left.copy_from_slice(&self.waveform_l[..WAVEFORM_SAMPLES]);
        data.waveform_right.copy_from_slice(&self.waveform_r[..WAVEFORM_SAMPLES]);
        data.spectrum_left.copy_from_slice(&self.spectrum_l[..SPECTRUM_SAMPLES]);
        data.spectrum_right.copy_from_slice(&self.spectrum_r[..SPECTRUM_SAMPLES]);

        data.bass = self.bass.current_relative();
        data.mid = self.middles.current_relative();
        data.treb = self.treble.current_relative();

        data.bass_att = self.bass.average_relative();
        data.mid_att = self.middles.average_relative();
        data.treb_att = self.treble.average_relative();

        data.vol = (data.bass + data.mid + data.treb) * 0.333;
        data.vol_att = (data.bass_att + data.mid_att + data.treb_att) * 0.333;

        data
    }

    fn copy_new_waveform_data(start: usize, source: &WaveformBuffer, dest: &mut WaveformBuffer) {
        for i in 0..AUDIO_BUFFER_SAMPLES {
            dest[i] = source[(start + i) % AUDIO_BUFFER_SAMPLES];
        }
    }

    fn update_spectrum(fft: &MilkdropFFT, waveform: &WaveformBuffer, spectrum: &mut SpectrumBuffer) {
        // Light damping (running 2-tap average) to reduce HF noise into the FFT.
        let mut samples = vec![0.0f32; AUDIO_BUFFER_SAMPLES];
        let mut old_i = 0usize;
        for i in 0..AUDIO_BUFFER_SAMPLES {
            samples[i] = 0.5 * (waveform[i] + waveform[old_i]);
            old_i = i;
        }

        let mut values = Vec::new();
        fft.time_to_frequency_domain(&samples, &mut values);
        spectrum.copy_from_slice(&values[..SPECTRUM_SAMPLES]);
    }
}
