//! Live audio capture via cpal, feeding [`pm_audio::PCM`]. Falls back to a
//! synthetic signal when no input device is available (e.g. headless / RDP).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use pm_audio::{FrameAudioData, WAVEFORM_SAMPLES, PCM};
use std::sync::{Arc, Mutex};

pub struct AudioInput {
    pcm: Arc<Mutex<PCM>>,
    /// Kept alive for the lifetime of capture; `None` when synthetic.
    _stream: Option<cpal::Stream>,
    synthetic: bool,
    source: String,
}

impl AudioInput {
    /// Try to open the default input device; on any failure, use synthetic audio.
    pub fn new() -> Self {
        let pcm = Arc::new(Mutex::new(PCM::new()));
        match Self::try_capture(pcm.clone()) {
            Ok((stream, name)) => {
                AudioInput { pcm, _stream: Some(stream), synthetic: false, source: name }
            }
            Err(e) => {
                eprintln!("audio: no live input ({e}); using synthetic signal");
                AudioInput { pcm, _stream: None, synthetic: true, source: "synthetic".into() }
            }
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    fn try_capture(pcm: Arc<Mutex<PCM>>) -> Result<(cpal::Stream, String), String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("no default input device")?;
        let name = "default input device".to_string();
        let config = device.default_input_config().map_err(|e| e.to_string())?;
        let sample_format = config.sample_format();
        let channels = config.channels() as u32;
        let stream_config: cpal::StreamConfig = config.into();

        let err_fn = |e| eprintln!("audio stream error: {e}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let pcm = pcm.clone();
                device.build_input_stream(
                    stream_config,
                    move |data: &[f32], _: &_| {
                        if let Ok(mut p) = pcm.lock() {
                            p.add_float(data, channels);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let pcm = pcm.clone();
                device.build_input_stream(
                    stream_config,
                    move |data: &[i16], _: &_| {
                        if let Ok(mut p) = pcm.lock() {
                            p.add_i16(data, channels);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, name))
    }

    /// Produce this frame's analyzed audio data.
    pub fn frame_data(&mut self, seconds_since_last: f64, frame: i32) -> FrameAudioData {
        if self.synthetic {
            // Feed a synthetic multi-tone waveform so visuals still react.
            let t = frame as f32 / 60.0;
            let mut samples = vec![0.0f32; WAVEFORM_SAMPLES * 2];
            for i in 0..WAVEFORM_SAMPLES {
                let p = i as f32 / WAVEFORM_SAMPLES as f32;
                let s = (p * 24.0 + t * 2.0).sin() * 0.4
                    + (p * 7.0 - t).sin() * 0.25
                    + (p * 53.0 + t * 0.5).sin() * 0.1;
                samples[i * 2] = s;
                samples[i * 2 + 1] = s * 0.8;
            }
            if let Ok(mut p) = self.pcm.lock() {
                p.add_float(&samples, 2);
            }
        }

        let mut pcm = self.pcm.lock().expect("pcm mutex");
        pcm.update_frame_audio_data(seconds_since_last, frame as u32);
        pcm.frame_audio_data()
    }
}
