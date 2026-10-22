//! The audio path: sixteen bands in series, then the output gain.
//!
//! Frequency, gain and Q glide (20 ms, in the log domain for frequency and Q)
//! with the coefficients recomputed every [`CHUNK`] samples. Changes of filter
//! type, slope or placement, and switching a band on or off, fade the band
//! out (to the dry signal), swap the structure while it is silent and fade
//! back in, so no edit clicks. Filters run in transposed direct form II in
//! f64. Nothing here allocates after construction.

use super::filter::{design, Band, Design, FilterType, Placement, Slope, MAX_SECTIONS};

pub const NUM_BANDS: usize = 16;
/// Coefficient update interval in samples.
pub const CHUNK: usize = 16;
const GLIDE_SECONDS: f64 = 0.02;
const FADE_SECONDS: f32 = 0.006;

type Shape = (FilterType, Slope, Placement);

fn shape(b: &Band) -> Shape {
    (b.kind, b.slope, b.place)
}

struct BandRt {
    target: Band,
    /// Structure currently running; replaced only while faded out.
    shape: Shape,
    /// Smoothed log2 frequency, gain in dB and log2 Q.
    lf: f64,
    gain: f64,
    lq: f64,
    design: Design,
    /// Filter state per channel (or mid/side part) and section.
    z: [[[f64; 2]; MAX_SECTIONS]; 2],
    /// Fade between the dry (0) and the filtered signal (1).
    wet: f32,
}

impl BandRt {
    fn new() -> Self {
        let b = Band::default();
        Self {
            target: b,
            shape: shape(&b),
            lf: (b.freq as f64).log2(),
            gain: b.gain as f64,
            lq: (b.q as f64).log2(),
            design: Design::BYPASS,
            z: [[[0.0; 2]; MAX_SECTIONS]; 2],
            wet: 0.0,
        }
    }

    fn current(&self) -> Band {
        Band {
            on: true,
            kind: self.shape.0,
            slope: self.shape.1,
            place: self.shape.2,
            freq: self.lf.exp2() as f32,
            gain: self.gain as f32,
            q: self.lq.exp2() as f32,
        }
    }

    /// Jumps to the target without gliding or fading.
    fn snap(&mut self, sr: f64) {
        let t = self.target;
        self.shape = shape(&t);
        self.lf = (t.freq as f64).log2();
        self.gain = t.gain as f64;
        self.lq = (t.q as f64).log2();
        self.design = design(&self.current(), sr);
        self.z = [[[0.0; 2]; MAX_SECTIONS]; 2];
    }

    /// Advances glides for a chunk of `len` samples and returns the wet level
    /// to fade towards.
    fn update(&mut self, len: usize, sr: f64) -> f32 {
        if self.wet == 0.0 && (shape(&self.target) != self.shape || !self.target.on) {
            // Silent: take the new structure and values at once.
            if shape(&self.target) != self.shape || self.current_differs() {
                self.snap(sr);
            }
        }
        if shape(&self.target) != self.shape {
            return 0.0;
        }
        if self.current_differs() {
            let k = 1.0 - (-(len as f64) / (sr * GLIDE_SECONDS)).exp();
            let t = &self.target;
            let glide = |cur: &mut f64, to: f64, eps: f64| {
                *cur += (to - *cur) * k;
                if (to - *cur).abs() < eps {
                    *cur = to;
                }
            };
            glide(&mut self.lf, (t.freq as f64).log2(), 1e-4);
            glide(&mut self.gain, t.gain as f64, 1e-3);
            glide(&mut self.lq, (t.q as f64).log2(), 1e-4);
            self.design = design(&self.current(), sr);
        }
        if self.target.on {
            1.0
        } else {
            0.0
        }
    }

    fn current_differs(&self) -> bool {
        let t = &self.target;
        self.lf != (t.freq as f64).log2() || self.gain != t.gain as f64 || self.lq != (t.q as f64).log2()
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32], to: f32, step: f32) {
        if self.wet == 0.0 && to == 0.0 {
            return;
        }
        let d = self.design;
        let n = d.n;
        let [z0, z1] = &mut self.z;
        let mut wet = self.wet;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            wet += (to - wet).clamp(-step, step);
            let w = wet as f64;
            let (xl, xr) = (*l as f64, *r as f64);
            let (yl, yr) = match self.shape.2 {
                Placement::Stereo => (cascade(&d, n, z0, xl), cascade(&d, n, z1, xr)),
                Placement::Left => (cascade(&d, n, z0, xl), xr),
                Placement::Right => (xl, cascade(&d, n, z0, xr)),
                Placement::Mid => {
                    let (m, s) = ((xl + xr) * 0.5, (xl - xr) * 0.5);
                    let m = cascade(&d, n, z0, m);
                    (m + s, m - s)
                }
                Placement::Side => {
                    let (m, s) = ((xl + xr) * 0.5, (xl - xr) * 0.5);
                    let s = cascade(&d, n, z0, s);
                    (m + s, m - s)
                }
            };
            *l = (xl + w * (yl - xl)) as f32;
            *r = (xr + w * (yr - xr)) as f32;
        }
        self.wet = wet;
        if wet == 0.0 {
            self.z = [[[0.0; 2]; MAX_SECTIONS]; 2];
        }
    }
}

#[inline(always)]
fn cascade(d: &Design, n: usize, z: &mut [[f64; 2]; MAX_SECTIONS], x: f64) -> f64 {
    let mut x = x;
    for (c, z) in d.sections[..n].iter().zip(z.iter_mut()) {
        let y = c.b0 * x + z[0];
        z[0] = c.b1 * x - c.a1 * y + z[1];
        z[1] = c.b2 * x - c.a2 * y;
        x = y;
    }
    x
}

pub struct Eq {
    sr: f64,
    bands: [BandRt; NUM_BANDS],
    out_target: f32,
    out: f32,
    fresh: bool,
}

impl Eq {
    pub fn new(sample_rate: f64) -> Self {
        Self {
            sr: sample_rate,
            bands: std::array::from_fn(|_| BandRt::new()),
            out_target: 1.0,
            out: 1.0,
            fresh: true,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f64) {
        self.sr = sample_rate;
        self.reset();
    }

    /// Clears filter state; the next [`Eq::set_params`] jumps to its values.
    pub fn reset(&mut self) {
        self.fresh = true;
    }

    /// New targets, in natural units. Call once per block before `process`.
    pub fn set_params(&mut self, bands: &[Band; NUM_BANDS], output_db: f32) {
        for (rt, b) in self.bands.iter_mut().zip(bands) {
            rt.target = *b;
        }
        self.out_target = 10f32.powf(output_db / 20.0);
        if self.fresh {
            self.fresh = false;
            self.out = self.out_target;
            for rt in &mut self.bands {
                rt.snap(self.sr);
                rt.wet = if rt.target.on { 1.0 } else { 0.0 };
            }
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let step = 1.0 / (FADE_SECONDS * self.sr as f32);
        let mut start = 0;
        let len = left.len().min(right.len());
        while start < len {
            let end = (start + CHUNK).min(len);
            let (l, r) = (&mut left[start..end], &mut right[start..end]);
            for rt in &mut self.bands {
                let to = rt.update(end - start, self.sr);
                rt.run(l, r, to, step);
            }
            let g0 = self.out;
            let g1 = if (self.out_target - g0).abs() < 1e-6 {
                self.out_target
            } else {
                g0 + (self.out_target - g0) * 0.08
            };
            let inc = (g1 - g0) / (end - start) as f32;
            for (i, (l, r)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
                let g = g0 + inc * (i + 1) as f32;
                *l *= g;
                *r *= g;
            }
            self.out = g1;
            start = end;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::filter::FLAT_Q;
    use std::f64::consts::PI;

    const SR: f64 = 48_000.0;

    fn sine(freq: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * freq * i as f64 / SR).sin() as f32 * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn bands_with(b: Band) -> [Band; NUM_BANDS] {
        let mut bands = [Band::default(); NUM_BANDS];
        bands[2] = b;
        bands
    }

    /// Gain in dB the EQ applies to a sine, measured after it settled.
    fn measured_db(eq: &mut Eq, freq: f64) -> f64 {
        let x = sine(freq, 24_000);
        let (mut l, mut r) = (x.clone(), x.clone());
        eq.process(&mut l, &mut r);
        20.0 * (rms(&l[12_000..]) / rms(&x[12_000..])).log10()
    }

    #[test]
    fn processed_gain_matches_the_designed_response() {
        let b = Band { on: true, kind: FilterType::Bell, freq: 2000.0, gain: 7.5, q: 2.0, ..Band::default() };
        let mut eq = Eq::new(SR);
        eq.set_params(&bands_with(b), 0.0);
        let want = design(&b, SR).response_db(2000.0, SR);
        assert!((measured_db(&mut eq, 2000.0) - want).abs() < 0.05);
        let want = design(&b, SR).response_db(500.0, SR);
        assert!((measured_db(&mut eq, 500.0) - want).abs() < 0.05);
    }

    #[test]
    fn output_gain_applies() {
        let mut eq = Eq::new(SR);
        eq.set_params(&[Band::default(); NUM_BANDS], -6.0);
        assert!((measured_db(&mut eq, 1000.0) + 6.0).abs() < 0.01);
    }

    #[test]
    fn placement_filters_only_its_part() {
        let b = Band { on: true, kind: FilterType::Bell, freq: 1000.0, gain: -12.0, q: 1.0, ..Band::default() };
        let x = sine(1000.0, 24_000);
        for (place, l_db, r_db) in [
            (Placement::Left, -12.0, 0.0),
            (Placement::Right, 0.0, -12.0),
            (Placement::Stereo, -12.0, -12.0),
        ] {
            let mut eq = Eq::new(SR);
            eq.set_params(&bands_with(Band { place, ..b }), 0.0);
            let (mut l, mut r) = (x.clone(), x.clone());
            eq.process(&mut l, &mut r);
            let db = |y: &[f32]| 20.0 * (rms(&y[12_000..]) / rms(&x[12_000..])).log10();
            assert!((db(&l) - l_db).abs() < 0.05 && (db(&r) - r_db).abs() < 0.05, "{place:?}");
        }
        // A mono signal has no side: a side band leaves it alone, a mid band cuts it.
        for (place, want) in [(Placement::Side, 0.0), (Placement::Mid, -12.0)] {
            let mut eq = Eq::new(SR);
            eq.set_params(&bands_with(Band { place, ..b }), 0.0);
            assert!((measured_db(&mut eq, 1000.0) - want).abs() < 0.05, "{place:?}");
        }
    }

    #[test]
    fn switching_type_fades_without_a_jump() {
        let b = Band { on: true, kind: FilterType::Bell, freq: 300.0, gain: 12.0, q: 1.0, ..Band::default() };
        let mut eq = Eq::new(SR);
        eq.set_params(&bands_with(b), 0.0);
        let x = sine(300.0, 48_000);
        let (mut l, mut r) = (x.clone(), x.clone());
        eq.process(&mut l[..24_000], &mut r[..24_000]);
        let cut = Band { kind: FilterType::LowCut, q: FLAT_Q, slope: Slope::Db48, ..b };
        eq.set_params(&bands_with(cut), 0.0);
        eq.process(&mut l[24_000..], &mut r[24_000..]);
        let max_step = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        // A 300 Hz sine at +12 dB moves at most ~0.08 per sample.
        assert!(max_step < 0.1, "{max_step}");
        assert!(rms(&l[40_000..]) < rms(&x[40_000..]) * 0.8);
    }

    #[test]
    fn disabled_bands_pass_audio_bit_exact() {
        let mut eq = Eq::new(SR);
        eq.set_params(&[Band::default(); NUM_BANDS], 0.0);
        let x = sine(440.0, 4800);
        let (mut l, mut r) = (x.clone(), x.clone());
        eq.process(&mut l, &mut r);
        assert_eq!(l, x);
        assert_eq!(r, x);
    }
}
