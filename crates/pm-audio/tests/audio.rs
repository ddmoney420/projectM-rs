//! Behavioral tests for `pm-audio`.

use pm_audio::{Band, Loudness, MilkdropFFT, AUDIO_BUFFER_SAMPLES, PCM, SPECTRUM_SAMPLES, WAVEFORM_SAMPLES};

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap()
}

fn mean(v: &[f32]) -> f32 {
    v.iter().sum::<f32>() / v.len() as f32
}

// ---------------------------------------------------------------- FFT --------

#[test]
fn fft_of_silence_is_zero() {
    let fft = MilkdropFFT::new(WAVEFORM_SAMPLES, SPECTRUM_SAMPLES, true, 1.0);
    let input = vec![0.0f32; AUDIO_BUFFER_SAMPLES];
    let mut spectrum = Vec::new();
    fft.time_to_frequency_domain(&input, &mut spectrum);

    assert_eq!(spectrum.len(), SPECTRUM_SAMPLES);
    assert!(spectrum.iter().all(|&v| v == 0.0));
}

#[test]
fn fft_of_sine_has_one_dominant_low_peak() {
    let fft = MilkdropFFT::new(WAVEFORM_SAMPLES, SPECTRUM_SAMPLES, true, 1.0);

    // 10 cycles across the 480 analyzed samples -> a low-frequency tone.
    let cycles = 10.0f32;
    let input: Vec<f32> = (0..AUDIO_BUFFER_SAMPLES)
        .map(|i| (std::f32::consts::TAU * cycles * i as f32 / WAVEFORM_SAMPLES as f32).sin())
        .collect();

    let mut spectrum = Vec::new();
    fft.time_to_frequency_domain(&input, &mut spectrum);

    let peak = argmax(&spectrum);
    let peak_val = spectrum[peak];

    // The DC bin is forced to zero by the equalization curve.
    assert_eq!(spectrum[0], 0.0);
    // A single low-frequency tone -> dominant peak well below the mid spectrum.
    assert!(peak < 60, "peak bin {peak} unexpectedly high");
    // Peak should tower over the average bin.
    assert!(peak_val > mean(&spectrum) * 10.0, "peak not dominant");
}

// ----------------------------------------------------------- Loudness --------

fn constant_spectrum(level: f32) -> [f32; SPECTRUM_SAMPLES] {
    [level; SPECTRUM_SAMPLES]
}

#[test]
fn loudness_relative_converges_to_one() {
    let mut bass = Loudness::new(Band::Bass);
    let spectrum = constant_spectrum(0.02);
    for frame in 0..4000 {
        bass.update(&spectrum, 1.0 / 60.0, frame);
    }
    // With a steady input, both averages track the current value -> ~1.0.
    assert!((bass.current_relative() - 1.0).abs() < 0.02, "got {}", bass.current_relative());
    assert!((bass.average_relative() - 1.0).abs() < 0.02);
}

#[test]
fn loudness_spikes_above_one_on_a_jump() {
    let mut bass = Loudness::new(Band::Bass);
    let quiet = constant_spectrum(0.02);
    for frame in 0..4000 {
        bass.update(&quiet, 1.0 / 60.0, frame);
    }
    // Sudden 4x jump in energy.
    let loud = constant_spectrum(0.08);
    bass.update(&loud, 1.0 / 60.0, 4000);
    assert!(bass.current_relative() > 1.5, "got {}", bass.current_relative());
}

#[test]
fn loudness_bands_cover_distinct_ranges() {
    // Bands span only the lower half of the spectrum, one sixth each:
    // bass = [0, 85), middles = [85, 170), treble = [170, 256).
    // Spike only inside the treble band -> treble moves, bass does not.
    let mut spectrum = [0.0f32; SPECTRUM_SAMPLES];
    for s in spectrum.iter_mut().take(SPECTRUM_SAMPLES / 2).skip(SPECTRUM_SAMPLES * 2 / 6) {
        *s = 1.0;
    }
    let mut bass = Loudness::new(Band::Bass);
    let mut treble = Loudness::new(Band::Treble);
    bass.update(&spectrum, 1.0 / 60.0, 0);
    treble.update(&spectrum, 1.0 / 60.0, 0);
    // Bass band saw no energy; treble band did.
    assert_eq!(bass.current_relative(), 1.0); // long avg ~0 -> defaults to 1.0
    assert!(treble.current_relative() != 1.0);
}

// ---------------------------------------------------------------- PCM --------

#[test]
fn pcm_i16_dc_passthrough_and_scaling() {
    // i16 value 256 maps to 128*256/32768 = 1.0.
    let mut pcm = PCM::new();
    let dc = vec![256i16; AUDIO_BUFFER_SAMPLES];
    pcm.add_i16(&dc, 1);
    pcm.update_frame_audio_data(1.0 / 60.0, 0);

    let data = pcm.frame_audio_data();
    // A constant signal must not be shifted by the aligner.
    for &v in data.waveform_left.iter() {
        assert!((v - 1.0).abs() < 1e-4, "expected 1.0, got {v}");
    }
}

#[test]
fn pcm_float_dc_scaling() {
    // f32 value 1.0 maps to 128.0 in the buffer.
    let mut pcm = PCM::new();
    let dc = vec![1.0f32; AUDIO_BUFFER_SAMPLES];
    pcm.add_float(&dc, 1);
    pcm.update_frame_audio_data(1.0 / 60.0, 0);

    let data = pcm.frame_audio_data();
    for &v in data.waveform_left.iter() {
        assert!((v - 128.0).abs() < 1e-2, "expected 128.0, got {v}");
    }
}

#[test]
fn pcm_mono_duplicates_to_both_channels() {
    let mut pcm = PCM::new();
    let sine: Vec<i16> = (0..AUDIO_BUFFER_SAMPLES)
        .map(|i| ((std::f32::consts::TAU * 8.0 * i as f32 / 480.0).sin() * 10000.0) as i16)
        .collect();
    pcm.add_i16(&sine, 1);
    pcm.update_frame_audio_data(1.0 / 60.0, 0);

    let data = pcm.frame_audio_data();
    assert_eq!(data.waveform_left, data.waveform_right);
    assert_eq!(data.spectrum_left, data.spectrum_right);
}

#[test]
fn pcm_sine_produces_spectral_energy_and_volume() {
    let mut pcm = PCM::new();
    let sine: Vec<i16> = (0..AUDIO_BUFFER_SAMPLES)
        .map(|i| ((std::f32::consts::TAU * 6.0 * i as f32 / 480.0).sin() * 12000.0) as i16)
        .collect();
    pcm.add_i16(&sine, 1);
    pcm.update_frame_audio_data(1.0 / 60.0, 0);

    let data = pcm.frame_audio_data();
    let energy: f32 = data.spectrum_left.iter().sum();
    assert!(energy > 0.0, "expected nonzero spectral energy");
    assert!(data.vol > 0.0, "expected nonzero volume");
}

#[test]
fn pcm_is_deterministic() {
    let make = || {
        let mut pcm = PCM::new();
        let sine: Vec<i16> = (0..AUDIO_BUFFER_SAMPLES)
            .map(|i| ((std::f32::consts::TAU * 7.0 * i as f32 / 480.0).sin() * 8000.0) as i16)
            .collect();
        // Two frames so the aligner has a previous reference to work against.
        pcm.add_i16(&sine, 1);
        pcm.update_frame_audio_data(1.0 / 60.0, 0);
        pcm.add_i16(&sine, 1);
        pcm.update_frame_audio_data(1.0 / 60.0, 1);
        pcm.frame_audio_data()
    };
    let a = make();
    let b = make();
    assert_eq!(a.waveform_left, b.waveform_left);
    assert_eq!(a.spectrum_left, b.spectrum_left);
    assert_eq!(a.bass, b.bass);
}
