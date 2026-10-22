//! Host parameters: sixteen bands of seven parameters each plus the output gain.
//! The IDs (`freq_3`, `type_1`, `output`, ...) are the automation and state
//! keys: never rename one. Choice parameters are `IntParam`s showing the names
//! from [`crate::dsp::filter`], so the DSP stays free of nih-plug types.

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};

use crate::dsp::eq::NUM_BANDS;
use crate::dsp::filter::{
    Band, FilterType, Placement, Slope, MAX_FREQ, MAX_GAIN, MAX_Q, MIN_FREQ, MIN_Q,
};

/// Default frequencies of the (initially off) bands.
pub const DEFAULT_FREQS: [f32; NUM_BANDS] = [
    30.0, 45.0, 70.0, 100.0, 150.0, 220.0, 330.0, 500.0, 750.0, 1100.0, 1700.0, 2500.0, 3700.0, 5500.0, 8200.0,
    12000.0,
];
pub const MAX_OUTPUT: f32 = 24.0;

/// One parameter of a band, for the editor's generic parameter I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Field {
    On,
    Kind,
    Freq,
    Gain,
    Q,
    Slope,
    Place,
}

#[derive(Params)]
pub struct BandParams {
    #[id = "on"]
    pub on: BoolParam,
    #[id = "type"]
    pub kind: IntParam,
    #[id = "freq"]
    pub freq: FloatParam,
    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "q"]
    pub q: FloatParam,
    #[id = "slope"]
    pub slope: IntParam,
    #[id = "place"]
    pub place: IntParam,
}

pub fn format_freq(hz: f32) -> String {
    if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else if hz >= 100.0 {
        format!("{hz:.0} Hz")
    } else {
        format!("{hz:.1} Hz")
    }
}

fn parse_freq(s: &str) -> Option<f32> {
    let t = s.trim().to_ascii_lowercase();
    let (num, scale) = match t.strip_suffix("khz").or_else(|| t.strip_suffix('k')) {
        Some(n) => (n, 1000.0),
        None => (t.trim_end_matches("hz"), 1.0),
    };
    num.trim().parse::<f32>().ok().map(|v| (v * scale).clamp(MIN_FREQ, MAX_FREQ))
}

pub fn format_db(db: f32) -> String {
    format!("{db:+.1} dB")
}

fn parse_number(s: &str) -> Option<f32> {
    s.trim().trim_end_matches(|c: char| c.is_alphabetic() || c == ' ').trim().parse().ok()
}

fn choice(name: String, default: i32, names: &'static [&'static str]) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min: 0, max: names.len() as i32 - 1 })
        .with_value_to_string(Arc::new(move |v| names[v.clamp(0, names.len() as i32 - 1) as usize].to_owned()))
        .with_string_to_value(Arc::new(move |s| {
            names.iter().position(|n| n.eq_ignore_ascii_case(s.trim())).map(|i| i as i32)
        }))
}

impl BandParams {
    pub fn new(index: usize) -> Self {
        let n = index + 1;
        Self {
            on: BoolParam::new(format!("Band {n} On"), false),
            kind: choice(format!("Band {n} Type"), FilterType::Bell as i32, &FilterType::NAMES),
            freq: FloatParam::new(
                format!("Band {n} Freq"),
                DEFAULT_FREQS[index],
                FloatRange::Skewed { min: MIN_FREQ, max: MAX_FREQ, factor: 0.2 },
            )
            .with_value_to_string(Arc::new(format_freq))
            .with_string_to_value(Arc::new(parse_freq)),
            gain: FloatParam::new(
                format!("Band {n} Gain"),
                0.0,
                FloatRange::Linear { min: -MAX_GAIN, max: MAX_GAIN },
            )
            .with_value_to_string(Arc::new(format_db))
            .with_string_to_value(Arc::new(parse_number)),
            q: FloatParam::new(format!("Band {n} Q"), 1.0, FloatRange::Skewed { min: MIN_Q, max: MAX_Q, factor: 0.35 })
                .with_value_to_string(Arc::new(|v| format!("{v:.2}")))
                .with_string_to_value(Arc::new(parse_number)),
            slope: choice(format!("Band {n} Slope"), Slope::Db12 as i32, &Slope::NAMES),
            place: choice(format!("Band {n} Placement"), Placement::Stereo as i32, &Placement::NAMES),
        }
    }

    /// Current values in natural units.
    pub fn band(&self) -> Band {
        Band {
            on: self.on.value(),
            kind: FilterType::from_index(self.kind.value()),
            freq: self.freq.value(),
            gain: self.gain.value(),
            q: self.q.value(),
            slope: Slope::from_index(self.slope.value()),
            place: Placement::from_index(self.place.value()),
        }
    }

    pub fn ptr(&self, field: Field) -> ParamPtr {
        match field {
            Field::On => self.on.as_ptr(),
            Field::Kind => self.kind.as_ptr(),
            Field::Freq => self.freq.as_ptr(),
            Field::Gain => self.gain.as_ptr(),
            Field::Q => self.q.as_ptr(),
            Field::Slope => self.slope.as_ptr(),
            Field::Place => self.place.as_ptr(),
        }
    }
}

#[derive(Params)]
pub struct GraphqParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    /// Editor settings saved with the project but not automatable: colour
    /// scheme, analyser mode and the graph's dB range (indices into the
    /// editor's tables).
    #[persist = "theme"]
    pub theme: Arc<AtomicU8>,
    #[persist = "analyzer"]
    pub analyzer: Arc<AtomicU8>,
    #[persist = "range"]
    pub range: Arc<AtomicU8>,
    /// Which automatic EQ made each band (`dsp::auto::Feature` tag, 0 =
    /// manual), so a new run replaces its own bands only. Editor thread only.
    #[persist = "auto"]
    pub auto_owner: Arc<Mutex<Vec<u8>>>,

    #[nested(array, group = "Band")]
    pub bands: [BandParams; NUM_BANDS],

    #[id = "output"]
    pub output: FloatParam,
}

impl Default for GraphqParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(crate::editor::WIDTH, crate::editor::HEIGHT),
            theme: Arc::new(AtomicU8::new(crate::editor::theme::DEFAULT_THEME)),
            analyzer: Arc::new(AtomicU8::new(crate::editor::DEFAULT_ANALYZER)),
            range: Arc::new(AtomicU8::new(crate::editor::DEFAULT_RANGE)),
            auto_owner: Arc::new(Mutex::new(vec![0; NUM_BANDS])),
            bands: std::array::from_fn(BandParams::new),
            output: FloatParam::new("Output", 0.0, FloatRange::Linear { min: -MAX_OUTPUT, max: MAX_OUTPUT })
                .with_value_to_string(Arc::new(format_db))
                .with_string_to_value(Arc::new(parse_number)),
        }
    }
}

impl GraphqParams {
    pub fn bands(&self) -> [Band; NUM_BANDS] {
        std::array::from_fn(|i| self.bands[i].band())
    }

    /// Owner tag of every band (a saved list of another length is padded).
    pub fn owners(&self) -> [u8; NUM_BANDS] {
        let v = self.auto_owner.lock().unwrap();
        std::array::from_fn(|i| v.get(i).copied().unwrap_or(0))
    }

    pub fn set_owners(&self, owners: [u8; NUM_BANDS]) {
        *self.auto_owner.lock().unwrap() = owners.to_vec();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable() {
        let p = GraphqParams::default();
        let ids: Vec<String> = p.param_map().into_iter().map(|(id, _, _)| id).collect();
        for id in ["on_1", "type_1", "freq_1", "gain_1", "q_1", "slope_1", "place_1", "freq_8", "output"] {
            assert!(ids.iter().any(|i| i == id), "missing {id}: {ids:?}");
        }
        assert_eq!(ids.len(), 7 * NUM_BANDS + 1);
    }

    #[test]
    fn text_round_trips() {
        let p = GraphqParams::default();
        let f = &p.bands[0].freq;
        assert_eq!(f.string_to_normalized_value("2.5 kHz").map(|n| f.preview_plain(n).round()), Some(2500.0));
        assert_eq!(format_freq(1234.0), "1.23 kHz");
        assert_eq!(format_freq(56.78), "56.8 Hz");
        let k = &p.bands[0].kind;
        assert_eq!(k.string_to_normalized_value("low cut").map(|n| k.preview_plain(n)), Some(3));
        assert_eq!(k.normalized_value_to_string(k.preview_normalized(7), false), "Tilt Shelf");
    }
}
