//! Spectrum analyser maths for the editor: FFT of the latest samples, sampled
//! at the graph's log-spaced frequencies, tilted so pink noise reads flat, and
//! a peak-hold-with-fall smoother. GUI thread only.

use super::fft::Fft;
use super::shared::AnalyzerRing;

pub const FFT_SIZE: usize = AnalyzerRing::SIZE;
/// Display tilt in dB per octave, pivoting at 1 kHz.
pub const TILT_DB_PER_OCT: f32 = 4.5;
/// How fast a displayed level falls after the signal drops, in dB per second.
pub const FALL_DB_PER_S: f32 = 48.0;
/// Time constant of a rising level, in seconds.
pub const RISE_SECONDS: f32 = 0.03;
/// Floor of the analyser in dB.
pub const FLOOR_DB: f32 = -120.0;

pub struct Analyzer {
    fft: Fft,
    window: Vec<f32>,
    samples: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Level per FFT bin in dBFS (a full-scale sine reads 0).
    bins: Vec<f32>,
}

impl Default for Analyzer {
    fn default() -> Self {
        let n = FFT_SIZE;
        Self {
            fft: Fft::new(n),
            window: (0..n)
                .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
                .collect(),
            samples: vec![0.0; n],
            re: vec![0.0; n],
            im: vec![0.0; n],
            bins: vec![FLOOR_DB; n / 2],
        }
    }
}

impl Analyzer {
    /// Analyses the latest samples of `ring` and writes the tilted level at
    /// each of `freqs` (ascending, Hz) into `out`.
    pub fn analyze(&mut self, ring: &AnalyzerRing, sample_rate: f32, freqs: &[f32], out: &mut [f32]) {
        ring.copy_latest(&mut self.samples);
        self.analyze_samples(sample_rate, freqs, out);
    }

    pub fn analyze_samples(&mut self, sample_rate: f32, freqs: &[f32], out: &mut [f32]) {
        let n = FFT_SIZE;
        for i in 0..n {
            self.re[i] = self.samples[i] * self.window[i];
            self.im[i] = 0.0;
        }
        self.fft.forward(&mut self.re, &mut self.im);
        // 2/n for the one-sided spectrum, 2 for the Hann window's coherent gain.
        let scale = 4.0 / n as f32;
        for (k, b) in self.bins.iter_mut().enumerate() {
            let power = (self.re[k] * self.re[k] + self.im[k] * self.im[k]) * scale * scale;
            *b = (10.0 * power.max(1e-18).log10()).max(FLOOR_DB);
        }

        let bin_hz = sample_rate / n as f32;
        let last = self.bins.len() - 1;
        let count = freqs.len();
        for (i, (&f, o)) in freqs.iter().zip(out.iter_mut()).enumerate() {
            let lo = if i > 0 { (freqs[i - 1] * f).sqrt() } else { f };
            let hi = if i + 1 < count { (freqs[i + 1] * f).sqrt() } else { f };
            let (k0, k1) = ((lo / bin_hz).ceil() as usize, (hi / bin_hz).floor() as usize);
            let level = if k1 > k0 && k0 <= last {
                // Several bins per point: the loudest one, so tones read at
                // their level however many bins a point covers.
                self.bins[k0..=k1.min(last)].iter().copied().fold(FLOOR_DB, f32::max)
            } else {
                // Fewer bins than points: interpolate between neighbours.
                let pos = (f / bin_hz).min(last as f32);
                let k = pos.floor() as usize;
                let t = pos - k as f32;
                let next = self.bins[(k + 1).min(last)];
                self.bins[k] * (1.0 - t) + next * t
            };
            let tilt = TILT_DB_PER_OCT * (f / 1000.0).log2();
            *o = if level <= FLOOR_DB { FLOOR_DB } else { level + tilt };
        }
    }
}

/// Moves a displayed level towards a new reading: rises quickly
/// ([`RISE_SECONDS`]), falls at [`FALL_DB_PER_S`].
pub fn smooth(shown: &mut [f32], fresh: &[f32], dt: f32) {
    let fall = FALL_DB_PER_S * dt;
    let rise = 1.0 - (-dt / RISE_SECONDS).exp();
    for (s, f) in shown.iter_mut().zip(fresh) {
        *s = if *f > *s { *s + (*f - *s) * rise } else { (*s - fall).max(*f) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_sine_reads_zero_db_at_its_frequency() {
        let sr = 48_000.0;
        let mut a = Analyzer::default();
        // Exactly on a bin, so there is no scalloping loss.
        let f = 171.0 * sr / FFT_SIZE as f32;
        for (i, s) in a.samples.iter_mut().enumerate() {
            *s = (std::f32::consts::TAU * f * i as f32 / sr).sin();
        }
        let freqs = [f / 4.0, f, f * 4.0];
        let mut out = [0.0; 3];
        a.analyze_samples(sr, &freqs, &mut out);
        let tilt = TILT_DB_PER_OCT * (f / 1000.0).log2();
        assert!((out[1] - tilt).abs() < 0.1, "{}", out[1] - tilt);
        assert!(out[0] < out[1] - 60.0 && out[2] < out[1] - 60.0);
    }

    #[test]
    fn silence_reads_the_floor() {
        let mut a = Analyzer::default();
        let mut out = [0.0; 2];
        a.analyze_samples(48_000.0, &[100.0, 10_000.0], &mut out);
        assert_eq!(out, [FLOOR_DB; 2]);
    }

    #[test]
    fn smoothing_rises_fast_and_falls_slowly() {
        let mut shown = [-60.0, -10.0];
        smooth(&mut shown, &[-20.0, -80.0], 0.5);
        assert!((shown[0] + 20.0).abs() < 1e-3);
        assert_eq!(shown[1], -10.0 - FALL_DB_PER_S * 0.5);
        let mut shown = [-60.0];
        smooth(&mut shown, &[-20.0], RISE_SECONDS);
        assert!((shown[0] - (-60.0 + 40.0 * (1.0 - (-1f32).exp()))).abs() < 1e-3);
    }
}
