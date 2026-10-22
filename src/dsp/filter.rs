//! Band settings and filter design: RBJ cookbook biquads, Butterworth cascades
//! for the cuts, and the exact magnitude response of what the audio path runs
//! (the graph draws this, so cramping near Nyquist is shown honestly).

use std::f64::consts::PI;

/// Most second-order sections one band needs (48 dB/oct cut = order 8).
pub const MAX_SECTIONS: usize = 4;

/// Frequency range of the bands and the graph.
pub const MIN_FREQ: f32 = 10.0;
pub const MAX_FREQ: f32 = 30_000.0;
pub const MIN_Q: f32 = 0.1;
pub const MAX_Q: f32 = 18.0;
pub const MAX_GAIN: f32 = 30.0;
/// Butterworth Q: the flat cut and the neutral shelf.
pub const FLAT_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterType {
    Bell = 0,
    LowShelf,
    HighShelf,
    LowCut,
    HighCut,
    Notch,
    BandPass,
    TiltShelf,
}

impl FilterType {
    pub const ALL: [FilterType; 8] = [
        FilterType::Bell,
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::LowCut,
        FilterType::HighCut,
        FilterType::Notch,
        FilterType::BandPass,
        FilterType::TiltShelf,
    ];
    pub const NAMES: [&'static str; 8] =
        ["Bell", "Low Shelf", "High Shelf", "Low Cut", "High Cut", "Notch", "Band Pass", "Tilt Shelf"];

    pub fn from_index(i: i32) -> Self {
        Self::ALL[i.clamp(0, 7) as usize]
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// Whether the gain parameter does anything.
    pub fn has_gain(self) -> bool {
        matches!(
            self,
            FilterType::Bell | FilterType::LowShelf | FilterType::HighShelf | FilterType::TiltShelf
        )
    }

    /// Whether the slope parameter does anything.
    pub fn has_slope(self) -> bool {
        matches!(self, FilterType::LowCut | FilterType::HighCut)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slope {
    Db6 = 0,
    Db12,
    Db18,
    Db24,
    Db36,
    Db48,
}

impl Slope {
    pub const ALL: [Slope; 6] = [Slope::Db6, Slope::Db12, Slope::Db18, Slope::Db24, Slope::Db36, Slope::Db48];
    pub const NAMES: [&'static str; 6] = ["6 dB/oct", "12 dB/oct", "18 dB/oct", "24 dB/oct", "36 dB/oct", "48 dB/oct"];

    pub fn from_index(i: i32) -> Self {
        Self::ALL[i.clamp(0, 5) as usize]
    }

    /// Filter order (6 dB/oct per order).
    pub fn order(self) -> usize {
        [1, 2, 3, 4, 6, 8][self as usize]
    }
}

/// Which part of the stereo signal a band filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Stereo = 0,
    Left,
    Right,
    Mid,
    Side,
}

impl Placement {
    pub const ALL: [Placement; 5] =
        [Placement::Stereo, Placement::Left, Placement::Right, Placement::Mid, Placement::Side];
    pub const NAMES: [&'static str; 5] = ["Stereo", "Left", "Right", "Mid", "Side"];

    pub fn from_index(i: i32) -> Self {
        Self::ALL[i.clamp(0, 4) as usize]
    }
}

/// One band in natural units, as the host parameters hold it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub on: bool,
    pub kind: FilterType,
    pub freq: f32,
    pub gain: f32,
    pub q: f32,
    pub slope: Slope,
    pub place: Placement,
}

impl Default for Band {
    fn default() -> Self {
        Self {
            on: false,
            kind: FilterType::Bell,
            freq: 1000.0,
            gain: 0.0,
            q: 1.0,
            slope: Slope::Db12,
            place: Placement::Stereo,
        }
    }
}

/// Normalised biquad coefficients (a0 = 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    fn norm(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        let r = 1.0 / a0;
        Coefs { b0: b0 * r, b1: b1 * r, b2: b2 * r, a1: a1 * r, a2: a2 * r }
    }

    /// |H(e^jw)|² at angular frequency `w` (radians per sample).
    pub fn power(&self, w: f64) -> f64 {
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        (nr * nr + ni * ni) / (dr * dr + di * di)
    }
}

/// The cascade one band runs: `n` sections, the rest unused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Design {
    pub n: usize,
    pub sections: [Coefs; MAX_SECTIONS],
}

impl Design {
    pub const BYPASS: Design = Design { n: 0, sections: [Coefs::IDENTITY; MAX_SECTIONS] };

    fn push(&mut self, c: Coefs) {
        self.sections[self.n] = c;
        self.n += 1;
    }

    /// Magnitude in dB at `freq` Hz.
    pub fn response_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let p: f64 = self.sections[..self.n].iter().map(|c| c.power(w)).product();
        10.0 * p.max(1.0e-30).log10()
    }
}

/// Highest band frequency the bilinear designs stay well behaved at.
pub fn max_freq(sample_rate: f64) -> f64 {
    (sample_rate * 0.49).min(MAX_FREQ as f64)
}

/// Designs the cascade for `band` in its current shape (frequency, gain, Q as
/// given; `on` is ignored, the caller bypasses).
pub fn design(band: &Band, sample_rate: f64) -> Design {
    let f = (band.freq as f64).clamp(MIN_FREQ as f64, max_freq(sample_rate));
    let q = (band.q as f64).clamp(MIN_Q as f64, MAX_Q as f64);
    let gain = (band.gain as f64).clamp(-(MAX_GAIN as f64), MAX_GAIN as f64);
    let w0 = 2.0 * PI * f / sample_rate;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = |q: f64| sw / (2.0 * q);
    let mut d = Design::BYPASS;
    match band.kind {
        FilterType::Bell => {
            let a = 10f64.powf(gain / 40.0);
            let al = alpha(q);
            d.push(Coefs::norm(1.0 + al * a, -2.0 * cw, 1.0 - al * a, 1.0 + al / a, -2.0 * cw, 1.0 - al / a));
        }
        FilterType::LowShelf => d.push(low_shelf(gain, cw, alpha(q))),
        FilterType::HighShelf => d.push(high_shelf(gain, cw, alpha(q))),
        FilterType::TiltShelf => {
            // High shelf by the full gain, the whole band lowered by half of
            // it: -g/2 below, +g/2 above, unity at the corner.
            let mut c = high_shelf(gain, cw, alpha(q));
            let k = 10f64.powf(-gain / 40.0);
            c.b0 *= k;
            c.b1 *= k;
            c.b2 *= k;
            d.push(c);
        }
        FilterType::Notch => {
            let al = alpha(q);
            d.push(Coefs::norm(1.0, -2.0 * cw, 1.0, 1.0 + al, -2.0 * cw, 1.0 - al));
        }
        FilterType::BandPass => {
            let al = alpha(q);
            d.push(Coefs::norm(al, 0.0, -al, 1.0 + al, -2.0 * cw, 1.0 - al));
        }
        FilterType::LowCut | FilterType::HighCut => {
            let high = band.kind == FilterType::HighCut;
            let order = band.slope.order();
            // Butterworth pole pairs; the Q parameter scales the most
            // resonant pair, so Q 0.71 is flat and higher values add a peak.
            let pairs = order / 2;
            for k in 0..pairs {
                // Angle of the pole pair from the negative real axis.
                let theta = if order.is_multiple_of(2) {
                    PI * (2 * k + 1) as f64 / (2 * order) as f64
                } else {
                    PI * (k + 1) as f64 / order as f64
                };
                let mut stage_q = 1.0 / (2.0 * theta.cos());
                if k == pairs - 1 {
                    stage_q *= q / FLAT_Q as f64;
                }
                let al = alpha(stage_q);
                d.push(if high {
                    Coefs::norm((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + al, -2.0 * cw, 1.0 - al)
                } else {
                    Coefs::norm((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + al, -2.0 * cw, 1.0 - al)
                });
            }
            if !order.is_multiple_of(2) {
                let k = (w0 / 2.0).tan();
                let a1 = (k - 1.0) / (k + 1.0);
                d.push(if high {
                    Coefs { b0: k / (1.0 + k), b1: k / (1.0 + k), b2: 0.0, a1, a2: 0.0 }
                } else {
                    Coefs { b0: 1.0 / (1.0 + k), b1: -1.0 / (1.0 + k), b2: 0.0, a1, a2: 0.0 }
                });
            }
        }
    }
    d
}

fn low_shelf(gain: f64, cw: f64, al: f64) -> Coefs {
    let a = 10f64.powf(gain / 40.0);
    let sa = 2.0 * a.sqrt() * al;
    Coefs::norm(
        a * ((a + 1.0) - (a - 1.0) * cw + sa),
        2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
        a * ((a + 1.0) - (a - 1.0) * cw - sa),
        (a + 1.0) + (a - 1.0) * cw + sa,
        -2.0 * ((a - 1.0) + (a + 1.0) * cw),
        (a + 1.0) + (a - 1.0) * cw - sa,
    )
}

fn high_shelf(gain: f64, cw: f64, al: f64) -> Coefs {
    let a = 10f64.powf(gain / 40.0);
    let sa = 2.0 * a.sqrt() * al;
    Coefs::norm(
        a * ((a + 1.0) + (a - 1.0) * cw + sa),
        -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
        a * ((a + 1.0) + (a - 1.0) * cw - sa),
        (a + 1.0) - (a - 1.0) * cw + sa,
        2.0 * ((a - 1.0) - (a + 1.0) * cw),
        (a + 1.0) - (a - 1.0) * cw - sa,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn band(kind: FilterType, freq: f32, gain: f32, q: f32) -> Band {
        Band { on: true, kind, freq, gain, q, ..Band::default() }
    }

    #[test]
    fn bell_peaks_at_its_gain() {
        let d = design(&band(FilterType::Bell, 1000.0, 9.0, 1.0), SR);
        assert!((d.response_db(1000.0, SR) - 9.0).abs() < 1e-6);
        assert!(d.response_db(30.0, SR).abs() < 0.1);
        assert!(d.response_db(15_000.0, SR).abs() < 0.2);
    }

    #[test]
    fn shelves_reach_their_gain_and_tilt_is_symmetric() {
        let lo = design(&band(FilterType::LowShelf, 500.0, -6.0, FLAT_Q), SR);
        assert!((lo.response_db(20.0, SR) + 6.0).abs() < 0.05);
        assert!((lo.response_db(500.0, SR) + 3.0).abs() < 0.05);
        assert!(lo.response_db(15_000.0, SR).abs() < 0.05);
        let hi = design(&band(FilterType::HighShelf, 2000.0, 8.0, FLAT_Q), SR);
        assert!((hi.response_db(20_000.0, SR) - 8.0).abs() < 0.1);
        let tilt = design(&band(FilterType::TiltShelf, 1000.0, 6.0, FLAT_Q), SR);
        assert!((tilt.response_db(20.0, SR) + 3.0).abs() < 0.05);
        assert!((tilt.response_db(20_000.0, SR) - 3.0).abs() < 0.1);
        assert!(tilt.response_db(1000.0, SR).abs() < 0.05);
    }

    #[test]
    fn cuts_have_their_slope_and_minus_3_db_at_the_corner() {
        for slope in Slope::ALL {
            let mut b = band(FilterType::LowCut, 1000.0, 0.0, FLAT_Q);
            b.slope = slope;
            let d = design(&b, SR);
            assert_eq!(d.n, slope.order().div_ceil(2));
            assert!((d.response_db(1000.0, SR) + 3.01).abs() < 0.05, "{slope:?}");
            // Two octaves below the corner the asymptote has taken over.
            let per_oct = d.response_db(62.5, SR) - d.response_db(125.0, SR);
            assert!((per_oct + 6.0 * slope.order() as f64).abs() < 0.3, "{slope:?} {per_oct}");
            assert!(d.response_db(10_000.0, SR).abs() < 0.05);
        }
        let mut b = band(FilterType::HighCut, 1000.0, 0.0, FLAT_Q);
        b.slope = Slope::Db24;
        let d = design(&b, SR);
        assert!((d.response_db(1000.0, SR) + 3.01).abs() < 0.05);
        assert!(d.response_db(4000.0, SR) < -45.0);
    }

    #[test]
    fn cut_q_adds_resonance() {
        let mut b = band(FilterType::LowCut, 1000.0, 0.0, 4.0);
        b.slope = Slope::Db12;
        let d = design(&b, SR);
        assert!((d.response_db(1000.0, SR) - 20.0 * 4f64.log10()).abs() < 0.05);
    }

    #[test]
    fn notch_and_band_pass() {
        let n = design(&band(FilterType::Notch, 1000.0, 0.0, 2.0), SR);
        assert!(n.response_db(1000.0, SR) < -100.0);
        assert!(n.response_db(100.0, SR).abs() < 0.1);
        let bp = design(&band(FilterType::BandPass, 1000.0, 0.0, 2.0), SR);
        assert!(bp.response_db(1000.0, SR).abs() < 1e-6);
        assert!(bp.response_db(100.0, SR) < -20.0);
    }

    #[test]
    fn extreme_settings_stay_finite() {
        for kind in FilterType::ALL {
            for freq in [MIN_FREQ, 1000.0, MAX_FREQ] {
                for q in [MIN_Q, MAX_Q] {
                    for slope in Slope::ALL {
                        let b = Band { slope, ..band(kind, freq, MAX_GAIN, q) };
                        for sr in [44_100.0, 192_000.0] {
                            let d = design(&b, sr);
                            for c in &d.sections[..d.n] {
                                assert!([c.b0, c.b1, c.b2, c.a1, c.a2].iter().all(|v| v.is_finite()));
                                // Stable: poles inside the unit circle.
                                assert!(c.a2.abs() < 1.0 && c.a1.abs() < 1.0 + c.a2, "{kind:?} {freq} {q}");
                            }
                        }
                    }
                }
            }
        }
    }
}
