//! Port of `Audio/MilkdropFFT.{hpp,cpp}` — the bespoke radix-2 FFT Milkdrop
//! uses for its spectrum analyzer, including the sine envelope window and the
//! log-scale equalization curve.
//!
//! Original: Copyright 2005-2013 Nullsoft, Inc. (BSD-style license, see source).

use std::ops::{Add, Mul, Sub};

const PI: f32 = std::f32::consts::PI;

/// Minimal `f32` complex number — kept local so this crate stays dependency-free.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Cf32 {
    re: f32,
    im: f32,
}

impl Cf32 {
    #[inline]
    fn new(re: f32, im: f32) -> Self {
        Cf32 { re, im }
    }
    /// `r * e^(i*theta)` — equivalent to C++ `std::polar`.
    #[inline]
    fn polar(r: f32, theta: f32) -> Self {
        Cf32 { re: r * theta.cos(), im: r * theta.sin() }
    }
    #[inline]
    fn abs(self) -> f32 {
        self.re.hypot(self.im)
    }
}

impl Add for Cf32 {
    type Output = Cf32;
    #[inline]
    fn add(self, o: Cf32) -> Cf32 {
        Cf32::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Cf32 {
    type Output = Cf32;
    #[inline]
    fn sub(self, o: Cf32) -> Cf32 {
        Cf32::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Cf32 {
    type Output = Cf32;
    #[inline]
    fn mul(self, o: Cf32) -> Cf32 {
        Cf32::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}

/// Milkdrop spectrum-analyzer FFT.
pub struct MilkdropFFT {
    samples_in: usize,
    num_frequencies: usize,
    bit_rev_table: Vec<usize>,
    envelope: Vec<f32>,
    equalize: Vec<f32>,
    cos_sin_table: Vec<Cf32>,
}

impl MilkdropFFT {
    /// * `samples_in` — number of waveform samples fed in.
    /// * `samples_out` — number of frequency samples out; the FFT runs at
    ///   `2 * samples_out` points, which **must be a power of two**.
    /// * `equalize` — roughly level basses vs. trebles on a log scale.
    /// * `envelope_power` — sine-window power; negative disables the window.
    pub fn new(samples_in: usize, samples_out: usize, equalize: bool, envelope_power: f32) -> Self {
        let num_frequencies = samples_out * 2;
        let mut fft = MilkdropFFT {
            samples_in,
            num_frequencies,
            bit_rev_table: Vec::new(),
            envelope: Vec::new(),
            equalize: Vec::new(),
            cos_sin_table: Vec::new(),
        };
        fft.init_bit_rev_table();
        fft.init_cos_sin_table();
        fft.init_envelope_table(envelope_power);
        fft.init_equalize_table(equalize);
        fft
    }

    /// Number of frequency samples produced — twice `samples_out`.
    pub fn num_frequencies(&self) -> usize {
        self.num_frequencies
    }

    fn init_envelope_table(&mut self, power: f32) {
        if power < 0.0 {
            self.envelope = vec![1.0; self.samples_in];
            return;
        }

        let multiplier = 1.0 / self.samples_in as f32 * 2.0 * PI;
        self.envelope.resize(self.samples_in, 0.0);

        if power == 1.0 {
            for i in 0..self.samples_in {
                self.envelope[i] = 0.5 + 0.5 * (i as f32 * multiplier - PI * 0.5).sin();
            }
        } else {
            for i in 0..self.samples_in {
                self.envelope[i] = (0.5 + 0.5 * (i as f32 * multiplier - PI * 0.5).sin()).powf(power);
            }
        }
    }

    fn init_equalize_table(&mut self, equalize: bool) {
        let half = self.num_frequencies / 2;
        if !equalize {
            self.equalize = vec![1.0; half];
            return;
        }

        let scaling = -0.02f32;
        let inv_half = 1.0 / half as f32;
        self.equalize.resize(half, 0.0);
        for i in 0..half {
            self.equalize[i] = scaling * ((half - i) as f32 * inv_half).ln();
        }
    }

    fn init_bit_rev_table(&mut self) {
        let n = self.num_frequencies;
        self.bit_rev_table = (0..n).collect();

        let mut j = 0usize;
        for i in 0..n {
            if j > i {
                self.bit_rev_table.swap(i, j);
            }
            let mut m = n >> 1;
            while m >= 1 && j >= m {
                j -= m;
                m >>= 1;
            }
            j += m;
        }
    }

    fn init_cos_sin_table(&mut self) {
        let mut tabsize = 0usize;
        let mut dftsize = 2usize;
        while dftsize <= self.num_frequencies {
            tabsize += 1;
            dftsize <<= 1;
        }

        self.cos_sin_table = Vec::with_capacity(tabsize);
        dftsize = 2;
        while dftsize <= self.num_frequencies {
            let theta = -2.0 * PI / dftsize as f32;
            self.cos_sin_table.push(Cf32::polar(1.0, theta));
            dftsize <<= 1;
        }
    }

    /// Convert time-domain `waveform` into magnitude `spectrum` (resized to
    /// `num_frequencies / 2`). On invalid input the output is cleared.
    pub fn time_to_frequency_domain(&self, waveform: &[f32], spectrum: &mut Vec<f32>) {
        if self.bit_rev_table.is_empty()
            || self.cos_sin_table.is_empty()
            || waveform.len() < self.samples_in
        {
            spectrum.clear();
            return;
        }

        let n = self.num_frequencies;

        // 1. Scatter windowed input into bit-reversed order (real part only).
        let mut data = vec![Cf32::default(); n];
        for (slot, &idx) in data.iter_mut().zip(self.bit_rev_table.iter()) {
            if idx < self.samples_in {
                slot.re = waveform[idx] * self.envelope[idx];
            }
        }

        // 2. In-place radix-2 decimation-in-time butterflies.
        let mut dft_size = 2usize;
        let mut octave = 0usize;
        while dft_size <= n {
            let mut w = Cf32::new(1.0, 0.0);
            let wp = self.cos_sin_table[octave];
            let hdft = dft_size >> 1;

            for m in 0..hdft {
                let mut i = m;
                while i < n {
                    let j = i + hdft;
                    let temp = data[j] * w;
                    data[j] = data[i] - temp;
                    data[i] = data[i] + temp;
                    i += dft_size;
                }
                w = w * wp;
            }

            dft_size <<= 1;
            octave += 1;
        }

        // 3. Equalized magnitudes for the lower half.
        let half = n / 2;
        spectrum.resize(half, 0.0);
        for i in 0..half {
            spectrum[i] = self.equalize[i] * data[i].abs();
        }
    }
}
