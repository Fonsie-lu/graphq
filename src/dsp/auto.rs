//! Automatic EQ from a 10 s capture of the plugin input. Two analyses, both
//! producing cuts only:
//!
//! - [`find_resonances`]: narrow, persistent spikes in the averaged spectrum
//!   (prominence over a median baseline, -3 dB width, present in most frames)
//!   become narrow bells.
//! - [`balance`]: the 1/3-octave spectrum is compared with pink noise (equal
//!   power per octave); regions above the median level are cut with broad
//!   bells or shelves, largest excess first.
//!
//! Both add the response of the bands they do not own (`others`) to the
//! measured spectrum first, so they see the signal as the rest of the EQ
//! already shapes it. Run resonances first, then balance.
//!
//! [`Capture`] is filled by the audio thread (lock-free, no allocation); the
//! analyses run on a background thread the editor starts ([`analyze`]).

use atomic_float::AtomicF32;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::fft::Fft;
use super::filter::{design, Band, Design, FilterType, Placement, Slope, FLAT_Q, MAX_Q};

pub const CAPTURE_SECONDS: f32 = 10.0;
pub const MAX_RESONANCES: usize = 6;
pub const MAX_BALANCE_BANDS: usize = 5;
/// Captures quieter than this (RMS, dBFS) are not analysed.
pub const MIN_LEVEL_DB: f32 = -60.0;

/// A resonance must stand this far above its surroundings (dB)...
const MIN_PROMINENCE: f64 = 6.0;
/// ...be at least this narrow (Q of its -3 dB width)...
const MIN_RESONANCE_Q: f64 = 3.0;
/// ...and stick out by 3 dB in at least this share of the frames.
const MIN_PERSISTENCE: f64 = 0.5;
const RESONANCE_RANGE: (f64, f64) = (40.0, 16_000.0);

/// Excess over the pink reference (dB) below which balance stops.
const MIN_EXCESS: f64 = 1.5;
/// Deepest cut of one balance band.
const MAX_BALANCE_CUT: f64 = 6.0;
/// Range the pink reference level is taken from, and the range balanced.
const REFERENCE_RANGE: (f64, f64) = (40.0, 12_000.0);
const BALANCE_RANGE: (f64, f64) = (25.0, 16_000.0);

/// Which analysis a band came from; stored per band (0 = manual).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    Resonances = 1,
    Balance = 2,
}

impl Feature {
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Feature::Resonances),
            2 => Some(Feature::Balance),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoError {
    /// The capture is quieter than [`MIN_LEVEL_DB`].
    NoSignal,
    /// Fewer samples than one analysis frame.
    TooShort,
}

/// Mono capture buffer. The audio thread appends until it is full.
pub struct Capture {
    data: Box<[AtomicF32]>,
    write: AtomicUsize,
    pub sample_rate: f32,
}

impl Capture {
    /// Allocates [`CAPTURE_SECONDS`] at `sample_rate` (not on the audio thread).
    pub fn new(sample_rate: f32) -> Self {
        let n = (sample_rate * CAPTURE_SECONDS).ceil() as usize;
        Self {
            data: (0..n).map(|_| AtomicF32::new(0.0)).collect(),
            write: AtomicUsize::new(0),
            sample_rate,
        }
    }

    pub fn reset(&self) {
        self.write.store(0, Ordering::Release);
    }

    /// Appends the mono sum of a stereo block, as far as there is room.
    pub fn push_stereo(&self, left: &[f32], right: &[f32]) {
        let n = self.data.len();
        let mut w = self.write.load(Ordering::Relaxed);
        if w >= n {
            return;
        }
        for (l, r) in left.iter().zip(right) {
            if w >= n {
                break;
            }
            self.data[w].store((l + r) * 0.5, Ordering::Relaxed);
            w += 1;
        }
        self.write.store(w, Ordering::Release);
    }

    pub fn progress(&self) -> f32 {
        self.write.load(Ordering::Acquire) as f32 / self.data.len() as f32
    }

    pub fn is_full(&self) -> bool {
        self.write.load(Ordering::Acquire) >= self.data.len()
    }

    /// The samples captured so far.
    pub fn samples(&self) -> Vec<f32> {
        let n = self.write.load(Ordering::Acquire).min(self.data.len());
        self.data[..n].iter().map(|v| v.load(Ordering::Relaxed)).collect()
    }
}

/// RMS level in dBFS.
pub fn level_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -200.0;
    }
    let ms = samples.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / samples.len() as f64;
    (10.0 * ms.max(1e-20).log10()) as f32
}

fn db(p: f64) -> f64 {
    10.0 * p.max(1e-30).log10()
}

/// FFT size giving about 3 Hz resolution.
fn fft_size(sample_rate: f32) -> usize {
    ((sample_rate / 3.0) as usize).next_power_of_two().clamp(4096, 65_536)
}

/// Hann-windowed power spectra of overlapping frames (hop n/4).
struct Frames {
    n: usize,
    fft: Fft,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    power: Vec<f64>,
}

impl Frames {
    fn new(n: usize) -> Self {
        Self {
            n,
            fft: Fft::new(n),
            window: (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect(),
            re: vec![0.0; n],
            im: vec![0.0; n],
            power: vec![0.0; n / 2],
        }
    }

    fn each(&mut self, samples: &[f32], mut f: impl FnMut(&[f64])) {
        let hop = self.n / 4;
        let mut start = 0;
        while start + self.n <= samples.len() {
            for i in 0..self.n {
                self.re[i] = samples[start + i] * self.window[i];
                self.im[i] = 0.0;
            }
            self.fft.forward(&mut self.re, &mut self.im);
            for (k, p) in self.power.iter_mut().enumerate() {
                *p = (self.re[k] as f64).powi(2) + (self.im[k] as f64).powi(2);
            }
            f(&self.power);
            start += hop;
        }
    }

    /// Mean power spectrum over all frames.
    fn average(&mut self, samples: &[f32]) -> Vec<f64> {
        let mut sum = vec![0.0; self.n / 2];
        let mut count = 0usize;
        self.each(samples, |p| {
            for (s, v) in sum.iter_mut().zip(p) {
                *s += v;
            }
            count += 1;
        });
        sum.iter_mut().for_each(|s| *s /= count.max(1) as f64);
        sum
    }
}

/// Summed response of the bands that are on.
struct Others(Vec<Design>, f64);

impl Others {
    fn new(bands: &[Band], sample_rate: f64) -> Self {
        Self(bands.iter().filter(|b| b.on).map(|b| design(b, sample_rate)).collect(), sample_rate)
    }

    fn db(&self, freq: f64) -> f64 {
        self.0.iter().map(|d| d.response_db(freq, self.1)).sum()
    }
}

fn check(samples: &[f32], n: usize) -> Result<(), AutoError> {
    if samples.len() < n {
        Err(AutoError::TooShort)
    } else if level_db(samples) < MIN_LEVEL_DB {
        Err(AutoError::NoSignal)
    } else {
        Ok(())
    }
}

fn median(v: &mut [f64]) -> f64 {
    let mid = v.len() / 2;
    *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1
}

struct Peak {
    freq: f64,
    prominence: f64,
    q: f64,
    bin: usize,
    /// Half width in bins, for the persistence check.
    half_width: f64,
}

/// Narrow cuts for the resonances in `samples`, at most [`MAX_RESONANCES`],
/// sorted by frequency. `others` are the bands already shaping the signal.
pub fn find_resonances(samples: &[f32], sample_rate: f32, others: &[Band]) -> Result<Vec<Band>, AutoError> {
    let n = fft_size(sample_rate);
    check(samples, n)?;
    let sr = sample_rate as f64;
    let bin_hz = sr / n as f64;
    let mut frames = Frames::new(n);
    let power = frames.average(samples);
    let others = Others::new(others, sr);

    let k_lo = (30.0 / bin_hz).ceil() as usize;
    let k_hi = ((18_000.0f64.min(sr * 0.45)) / bin_hz).floor() as usize;
    // Spectrum as the EQ's other bands shape it.
    let mut a = vec![f64::NEG_INFINITY; n / 2];
    for k in k_lo..=k_hi {
        a[k] = db(power[k]) + others.db(k as f64 * bin_hz);
    }

    // Median baseline on a 1/48-octave grid, ±1/3 octave (at least ±16 bins).
    let mut grid = Vec::new();
    let mut f = 30.0;
    let mut scratch = Vec::new();
    while f <= k_hi as f64 * bin_hz {
        let lo = ((f / 2f64.powf(1.0 / 3.0) / bin_hz).floor() as usize).min(((f / bin_hz) as usize).saturating_sub(16));
        let hi = ((f * 2f64.powf(1.0 / 3.0) / bin_hz).ceil() as usize).max((f / bin_hz) as usize + 16);
        scratch.clear();
        scratch.extend_from_slice(&a[lo.max(k_lo)..=hi.min(k_hi)]);
        grid.push((f.log2(), median(&mut scratch)));
        f *= 2f64.powf(1.0 / 48.0);
    }
    let baseline = |k: usize| -> f64 {
        let x = (k as f64 * bin_hz).log2();
        let i = grid.partition_point(|(gx, _)| *gx < x).clamp(1, grid.len() - 1);
        let ((x0, y0), (x1, y1)) = (grid[i - 1], grid[i]);
        y0 + (y1 - y0) * ((x - x0) / (x1 - x0)).clamp(0.0, 1.0)
    };

    let mut peaks = Vec::new();
    let r_lo = ((RESONANCE_RANGE.0 / bin_hz).ceil() as usize).max(k_lo + 3);
    let r_hi = ((RESONANCE_RANGE.1 / bin_hz).floor() as usize).min(k_hi - 3);
    for k in r_lo..=r_hi {
        if (k - 3..=k + 3).any(|j| j != k && a[j] >= a[k]) {
            continue;
        }
        let prominence = a[k] - baseline(k);
        if prominence < MIN_PROMINENCE {
            continue;
        }
        // -3 dB crossings, searched over at most half an octave each way.
        let level = a[k] - 3.0;
        let reach = ((k as f64 * 0.3) as usize).max(4);
        let mut j = k;
        while j > k_lo && k - j < reach && a[j - 1] > level {
            j -= 1;
        }
        let left = if a[j - 1] <= level { j as f64 - (a[j] - level) / (a[j] - a[j - 1]) } else { j as f64 };
        let mut j = k;
        while j < k_hi && j - k < reach && a[j + 1] > level {
            j += 1;
        }
        let right = if a[j + 1] <= level { j as f64 + (a[j] - level) / (a[j] - a[j + 1]) } else { j as f64 };
        // Parabolic interpolation of the peak position.
        let (y0, y1, y2) = (a[k - 1], a[k], a[k + 1]);
        let delta = (0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2)).clamp(-0.5, 0.5);
        let freq = (k as f64 + delta) * bin_hz;
        let q = freq / ((right - left).max(1.0) * bin_hz);
        if q < MIN_RESONANCE_Q {
            continue;
        }
        peaks.push(Peak { freq, prominence, q, bin: k, half_width: (right - left) * 0.5 });
    }

    // Persistence: the peak must stand out in most frames, not just on average.
    let mut hits = vec![0usize; peaks.len()];
    let mut count = 0usize;
    frames.each(samples, |p| {
        count += 1;
        for (peak, hit) in peaks.iter().zip(hits.iter_mut()) {
            let inner = (peak.half_width.ceil() as usize + 2).max(3);
            let outer = inner + ((peak.bin as f64 * 0.12) as usize).max(8);
            let ring = (peak.bin.saturating_sub(outer)..peak.bin.saturating_sub(inner))
                .chain(peak.bin + inner..(peak.bin + outer).min(p.len()));
            let (sum, m) = ring.fold((0.0, 0usize), |(s, m), j| (s + p[j], m + 1));
            if db(p[peak.bin]) - db(sum / m.max(1) as f64) > 3.0 {
                *hit += 1;
            }
        }
    });
    let mut kept: Vec<Peak> = peaks
        .into_iter()
        .zip(hits)
        .filter(|(_, h)| *h as f64 >= MIN_PERSISTENCE * count.max(1) as f64)
        .map(|(p, _)| p)
        .collect();

    // Strongest first; one cut per 1/12 octave.
    kept.sort_by(|a, b| b.prominence.total_cmp(&a.prominence));
    let mut chosen: Vec<Peak> = Vec::new();
    for p in kept {
        if chosen.len() == MAX_RESONANCES {
            break;
        }
        if chosen.iter().all(|c| (p.freq / c.freq).log2().abs() > 1.0 / 12.0) {
            chosen.push(p);
        }
    }
    chosen.sort_by(|a, b| a.freq.total_cmp(&b.freq));
    Ok(chosen
        .iter()
        .map(|p| Band {
            on: true,
            kind: FilterType::Bell,
            freq: p.freq as f32,
            // Down to the surrounding level; the measured prominence already
            // reads a little low for wider resonances.
            gain: -p.prominence.clamp(2.0, 12.0) as f32,
            q: p.q.clamp(4.0, MAX_Q as f64) as f32,
            slope: Slope::Db12,
            place: Placement::Stereo,
        })
        .collect())
}

/// Broad cuts that bring the spectrum of `samples` towards pink noise, at
/// most [`MAX_BALANCE_BANDS`], largest excess first. Never boosts.
pub fn balance(samples: &[f32], sample_rate: f32, others: &[Band]) -> Result<Vec<Band>, AutoError> {
    let n = fft_size(sample_rate);
    check(samples, n)?;
    let sr = sample_rate as f64;
    let bin_hz = sr / n as f64;
    let power = Frames::new(n).average(samples);
    let others = Others::new(others, sr);

    // 1/3-octave levels every 1/12 octave, tilted +3 dB/oct so pink is flat.
    let top = BALANCE_RANGE.1.min(sr * 0.45);
    let freqs: Vec<f64> = (0..)
        .map(|i| BALANCE_RANGE.0 * 2f64.powf(i as f64 / 12.0))
        .take_while(|f| *f <= top)
        .collect();
    let level: Vec<f64> = freqs
        .iter()
        .map(|&f| {
            let lo = (f / 2f64.powf(1.0 / 6.0) / bin_hz).ceil() as usize;
            let hi = ((f * 2f64.powf(1.0 / 6.0) / bin_hz).floor() as usize).max(lo);
            let span = &power[lo.max(1)..=hi.min(power.len() - 1)];
            let mean = span.iter().sum::<f64>() / span.len() as f64;
            db(mean) + others.db(f) + 10.0 * (f / 1000.0).log10()
        })
        .collect();
    let mut reference: Vec<f64> = freqs
        .iter()
        .zip(&level)
        .filter(|(f, _)| (REFERENCE_RANGE.0..=REFERENCE_RANGE.1).contains(*f))
        .map(|(_, l)| *l)
        .collect();
    let reference = median(&mut reference);
    let mut excess: Vec<f64> = level.iter().map(|l| (l - reference).max(0.0)).collect();

    let last = freqs.len() - 1;
    let mut bands = Vec::new();
    while bands.len() < MAX_BALANCE_BANDS {
        let (peak, e) = excess.iter().enumerate().fold((0, 0.0), |m, (i, e)| if *e > m.1 { (i, *e) } else { m });
        if e < MIN_EXCESS {
            break;
        }
        let half = e * 0.5;
        let mut lo = peak;
        while lo > 0 && excess[lo - 1] > half {
            lo -= 1;
        }
        let mut hi = peak;
        while hi < last && excess[hi + 1] > half {
            hi += 1;
        }
        let gain = -e.min(MAX_BALANCE_CUT) as f32;
        let band = if lo == 0 && freqs[peak] < 200.0 {
            // Excess down to the bottom: a low shelf with its half-gain
            // corner where the excess halves.
            Band { kind: FilterType::LowShelf, freq: freqs[hi] as f32, q: FLAT_Q, ..cut(gain) }
        } else if hi == last && freqs[peak] > 4000.0 {
            Band { kind: FilterType::HighShelf, freq: freqs[lo] as f32, q: FLAT_Q, ..cut(gain) }
        } else {
            // Width between the half-excess points, as the RBJ bell's
            // bandwidth (between its half-gain points).
            let octaves = ((freqs[hi] / freqs[lo]).log2() + 1.0 / 12.0).clamp(0.4, 4.0);
            let q = 2f64.powf(octaves).sqrt() / (2f64.powf(octaves) - 1.0);
            Band { kind: FilterType::Bell, freq: freqs[peak] as f32, q: q.clamp(0.3, 4.0) as f32, ..cut(gain) }
        };
        let d = design(&band, sr);
        for (e, f) in excess.iter_mut().zip(&freqs) {
            *e = (*e + d.response_db(*f, sr)).max(0.0);
        }
        bands.push(band);
    }
    Ok(bands)
}

/// Runs `feature`'s analysis.
pub fn analyze(feature: Feature, samples: &[f32], sample_rate: f32, others: &[Band]) -> Result<Vec<Band>, AutoError> {
    match feature {
        Feature::Resonances => find_resonances(samples, sample_rate, others),
        Feature::Balance => balance(samples, sample_rate, others),
    }
}

fn cut(gain: f32) -> Band {
    Band { on: true, gain, slope: Slope::Db12, place: Placement::Stereo, ..Band::default() }
}

/// Pink noise (Paul Kellet's refined filter, ±0.05 dB above 9 Hz at 44.1 kHz).
/// Used by the tests and the preview's demo signal.
pub fn pink_noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed.max(1);
    let mut b = [0.0f32; 7];
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let w = (s >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            b[0] = 0.99886 * b[0] + w * 0.055_517_9;
            b[1] = 0.99332 * b[1] + w * 0.075_075_9;
            b[2] = 0.96900 * b[2] + w * 0.153_852;
            b[3] = 0.86650 * b[3] + w * 0.310_485_6;
            b[4] = 0.55000 * b[4] + w * 0.532_952_2;
            b[5] = -0.7616 * b[5] - w * 0.016_898;
            let out = b.iter().sum::<f32>() + w * 0.5362;
            b[6] = w * 0.115_926;
            out * 0.05
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::eq::{Eq, NUM_BANDS};
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;
    const LEN: usize = 480_000;

    fn add_sine(x: &mut [f32], freq: f32, amp: f32, from: usize, to: usize) {
        for (i, v) in x.iter_mut().enumerate().take(to).skip(from) {
            *v += amp * (TAU * freq * i as f32 / SR).sin();
        }
    }

    /// Runs `x` through an EQ with one band.
    fn shaped(x: &[f32], band: Band) -> Vec<f32> {
        let mut bands = [Band::default(); NUM_BANDS];
        bands[0] = band;
        let mut eq = Eq::new(SR as f64);
        eq.set_params(&bands, 0.0);
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        eq.process(&mut l, &mut r);
        l
    }

    #[test]
    fn capture_fills_once_and_resets() {
        let c = Capture::new(100.0);
        c.push_stereo(&[1.0; 600], &[0.0; 600]);
        c.push_stereo(&[1.0; 600], &[0.0; 600]);
        assert!(c.is_full());
        assert_eq!(c.samples().len(), 1000);
        assert_eq!(c.samples()[999], 0.5);
        c.reset();
        assert_eq!(c.progress(), 0.0);
    }

    #[test]
    fn pink_noise_has_no_resonances_and_needs_no_balance() {
        for seed in [1, 7, 19, 23, 42, 77] {
            let x = pink_noise(LEN, seed);
            assert_eq!(find_resonances(&x, SR, &[]).unwrap(), vec![], "seed {seed}");
            let b = balance(&x, SR, &[]).unwrap();
            assert!(b.is_empty(), "seed {seed}: {b:?}");
        }
    }

    /// Prints what the analyses make of a few signals, for tuning:
    /// `cargo test --lib print_examples -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn print_examples() {
        let boost = Band { on: true, kind: FilterType::LowShelf, freq: 200.0, gain: 8.0, q: FLAT_Q, ..Band::default() };
        let x = shaped(&pink_noise(LEN, 11), boost);
        eprintln!("bass: {:#?}", balance(&x, SR, &[]).unwrap());
        let tilt = Band { on: true, kind: FilterType::TiltShelf, freq: 1000.0, gain: -9.0, q: FLAT_Q, ..Band::default() };
        let x = shaped(&pink_noise(LEN, 12), tilt);
        eprintln!("dark tilt: {:#?}", balance(&x, SR, &[]).unwrap());
        let mut x = pink_noise(LEN, 3);
        add_sine(&mut x, 1000.0, 0.02, 0, LEN);
        add_sine(&mut x, 3150.0, 0.01, 0, LEN);
        add_sine(&mut x, 82.0, 0.03, 0, LEN);
        eprintln!("res: {:#?}", find_resonances(&x, SR, &[]).unwrap());
        // A resonant room-mode-like peak (bell +12 dB, Q 8 at 120 Hz).
        let mode = Band { on: true, kind: FilterType::Bell, freq: 120.0, gain: 12.0, q: 8.0, ..Band::default() };
        let x = shaped(&pink_noise(LEN, 4), mode);
        eprintln!("mode: {:#?}", find_resonances(&x, SR, &[]).unwrap());
    }

    #[test]
    fn steady_tones_are_found_and_cut_narrowly() {
        let mut x = pink_noise(LEN, 3);
        add_sine(&mut x, 1000.0, 0.02, 0, LEN);
        add_sine(&mut x, 3150.0, 0.01, 0, LEN);
        let r = find_resonances(&x, SR, &[]).unwrap();
        assert_eq!(r.len(), 2, "{r:?}");
        for (band, f) in r.iter().zip([1000.0, 3150.0]) {
            assert!((band.freq / f - 1.0).abs() < 0.005, "{band:?}");
            assert!(band.gain < -2.0 && band.gain >= -12.0);
            assert!(band.q >= 4.0);
            assert_eq!(band.kind, FilterType::Bell);
        }
    }

    #[test]
    fn a_short_note_is_not_a_resonance() {
        let mut x = pink_noise(LEN, 5);
        add_sine(&mut x, 1500.0, 0.03, 0, LEN * 3 / 10);
        assert_eq!(find_resonances(&x, SR, &[]).unwrap(), vec![]);
    }

    #[test]
    fn existing_cuts_hide_a_resonance() {
        let mut x = pink_noise(LEN, 9);
        add_sine(&mut x, 800.0, 0.01, 0, LEN);
        let first = find_resonances(&x, SR, &[]).unwrap();
        assert_eq!(first.len(), 1);
        let notch = Band { kind: FilterType::Notch, freq: 800.0, q: 8.0, ..cut(0.0) };
        assert_eq!(find_resonances(&x, SR, &[notch]).unwrap(), vec![]);
    }

    #[test]
    fn heavy_bass_is_cut_back_towards_pink() {
        let boost = Band { on: true, kind: FilterType::LowShelf, freq: 200.0, gain: 8.0, q: FLAT_Q, ..Band::default() };
        let x = shaped(&pink_noise(LEN, 11), boost);
        let b = balance(&x, SR, &[]).unwrap();
        assert!(!b.is_empty());
        assert!(b.iter().all(|band| band.gain < 0.0 && band.gain >= -6.0));
        assert!(b[0].freq < 400.0, "{b:?}");
        // Applied together they undo most of the boost at 60 Hz.
        let total: f64 = b.iter().map(|band| design(band, SR as f64).response_db(60.0, SR as f64)).sum();
        assert!(total < -6.0, "{total} {b:?}");
        // With the boost already undone by another band nothing is left to do.
        let undo = Band { gain: -8.0, ..boost };
        assert!(balance(&x, SR, &[undo]).unwrap().is_empty());
    }

    #[test]
    fn harsh_top_gets_a_high_cut_band() {
        let boost = Band { on: true, kind: FilterType::Bell, freq: 3000.0, gain: 7.0, q: 1.0, ..Band::default() };
        let x = shaped(&pink_noise(LEN, 13), boost);
        let b = balance(&x, SR, &[]).unwrap();
        assert!(b.iter().any(|band| band.kind == FilterType::Bell && (band.freq / 3000.0).log2().abs() < 0.3), "{b:?}");
        assert!(b.iter().all(|band| band.gain < 0.0));
    }

    #[test]
    fn silence_is_rejected() {
        let x = vec![0.0; LEN];
        assert_eq!(find_resonances(&x, SR, &[]), Err(AutoError::NoSignal));
        assert_eq!(balance(&x[..100], SR, &[]), Err(AutoError::TooShort));
    }
}
