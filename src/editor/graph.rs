//! The graph: frequency grid, spectrum analyser, band and summed curves, the
//! draggable band nodes and the output meter.
//!
//! Mouse: drag a node to move frequency and gain (Shift: fine), wheel on a
//! node for Q, double-click empty space to add a band (a cut near either
//! end), double-click a node to remove it, right-click a node for its type.

use nih_plug_egui::egui::{pos2, vec2, Align2, Color32, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind};
use std::sync::atomic::Ordering::Relaxed;

use super::widgets::{fill_to, Pid, G};
use super::{place_tag, popup_origin, ControlKind, BW, RANGES, RANGE_STEPS};
use crate::dsp::auto::Feature;
use crate::dsp::eq::NUM_BANDS;
use crate::dsp::filter::{design, Band, FilterType, Slope, FLAT_Q, MAX_FREQ, MAX_GAIN, MAX_Q, MIN_FREQ, MIN_Q};
use crate::dsp::spectrum::{self, Analyzer, FLOOR_DB};
use crate::params::{format_db, format_freq, Field};

/// Graph area in canvas coordinates.
pub(crate) const GX0: f32 = 16.0;
pub(crate) const GX1: f32 = BW - 64.0;
pub(crate) const GY0: f32 = 76.0;
pub(crate) const GY1: f32 = 458.0;
/// Space between the ±range lines and the graph's top and bottom edges.
const MARGIN: f32 = 14.0;
/// Points along the frequency axis for curves and the analyser.
const POINTS: usize = 360;
/// Analyser level at the top and bottom edge of the graph.
const ANA_TOP: f32 = 6.0;
const ANA_BOTTOM: f32 = -90.0;
/// Meter scale.
const METER_TOP: f32 = 6.0;
const METER_BOTTOM: f32 = -60.0;
const NODE_R: f32 = 9.0;

pub(crate) struct GraphState {
    freqs: Vec<f32>,
    analyzer: Analyzer,
    fresh: Vec<f32>,
    /// Displayed analyser levels before and after the EQ.
    spectrum: [Vec<f32>; 2],
    /// Displayed output meter levels in dB.
    meter: [f32; 2],
    last_time: Option<f64>,
    drag: Option<NodeDrag>,
}

#[derive(Clone, Copy)]
struct NodeDrag {
    band: usize,
    /// Node position (canvas) when the drag started.
    start: Pos2,
    gain: bool,
}

impl Default for GraphState {
    fn default() -> Self {
        let freqs = (0..POINTS)
            .map(|i| MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(i as f32 / (POINTS - 1) as f32))
            .collect();
        Self {
            freqs,
            analyzer: Analyzer::default(),
            fresh: vec![FLOOR_DB; POINTS],
            spectrum: [vec![FLOOR_DB; POINTS], vec![FLOOR_DB; POINTS]],
            meter: [METER_BOTTOM; 2],
            last_time: None,
            drag: None,
        }
    }
}

/// Canvas x of a frequency.
pub(crate) fn x_of(freq: f32) -> f32 {
    GX0 + (freq / MIN_FREQ).ln() / (MAX_FREQ / MIN_FREQ).ln() * (GX1 - GX0)
}

pub(crate) fn freq_at(x: f32) -> f32 {
    MIN_FREQ * (MAX_FREQ / MIN_FREQ).powf(((x - GX0) / (GX1 - GX0)).clamp(0.0, 1.0))
}

/// Canvas y of a gain at the graph range `range` (± dB).
pub(crate) fn y_of(db: f32, range: f32) -> f32 {
    let mid = (GY0 + GY1) * 0.5;
    mid - db / range * ((GY1 - GY0) * 0.5 - MARGIN)
}

pub(crate) fn db_at(y: f32, range: f32) -> f32 {
    let mid = (GY0 + GY1) * 0.5;
    (mid - y) / ((GY1 - GY0) * 0.5 - MARGIN) * range
}

fn ana_y(db: f32) -> f32 {
    GY0 + (ANA_TOP - db) / (ANA_TOP - ANA_BOTTOM) * (GY1 - GY0)
}

impl G<'_, '_> {
    pub(crate) fn range(&self) -> f32 {
        RANGES[(self.p.range.load(Relaxed) as usize).min(RANGES.len() - 1)]
    }

    pub(crate) fn graph(&mut self) {
        let cv = self.cv;
        let th = self.th;
        let range = self.range();
        let area = cv.r(GX0, GY0, GX1 - GX0, GY1 - GY0);
        self.pt.rect_filled(area, cv.round(4.0), th.display_bg);

        let now = self.now();
        let dt = self.st.graph.last_time.map_or(0.0, |t| (now - t).clamp(0.0, 0.25)) as f32;
        self.st.graph.last_time = Some(now);

        self.grid(range);
        self.analyzer(dt);

        let sr = self.st.shared.sample_rate.load(Relaxed) as f64;
        let bands: Vec<Band> = (0..NUM_BANDS).map(|b| self.band(b)).collect();
        let painter = self.pt.with_clip_rect(area);
        let mut total = vec![0.0f32; POINTS];
        let zero = cv.p(0.0, y_of(0.0, range)).y;
        let to_screen = |i: usize, db: f32, freqs: &[f32]| {
            let y = y_of(db.clamp(-range * 4.0, range * 4.0), range);
            cv.p(x_of(freqs[i]), y)
        };
        for (b, band) in bands.iter().enumerate() {
            if !band.on {
                continue;
            }
            let d = design(band, sr);
            let freqs = &self.st.graph.freqs;
            let resp: Vec<f32> = freqs.iter().map(|&f| d.response_db(f as f64, sr) as f32).collect();
            for (t, r) in total.iter_mut().zip(&resp) {
                *t += r;
            }
            let pts: Vec<Pos2> = resp.iter().enumerate().map(|(i, &db)| to_screen(i, db, freqs)).collect();
            let color = th.band(b);
            if b == self.st.selected {
                fill_to(&painter, &pts, zero, color.gamma_multiply(0.16));
            }
            painter.add(Shape::line(pts, Stroke::new(1.2 * cv.s, color.gamma_multiply(0.55))));
        }
        let freqs = &self.st.graph.freqs;
        let pts: Vec<Pos2> = total.iter().enumerate().map(|(i, &db)| to_screen(i, db, freqs)).collect();
        painter.add(Shape::line(pts, Stroke::new(2.4 * cv.s, th.curve)));

        self.nodes(&bands, range);
        self.meter(dt);
        self.pt.rect_stroke(area, cv.round(4.0), cv.stroke(1.0, th.display_frame), StrokeKind::Inside);
    }

    fn grid(&self, range: f32) {
        let cv = self.cv;
        let th = self.th;
        let mut decade = 10.0;
        while decade < MAX_FREQ {
            for k in 1..10 {
                let f = decade * k as f32;
                if !(MIN_FREQ..=MAX_FREQ).contains(&f) {
                    continue;
                }
                let x = x_of(f);
                let major = k == 1;
                let color = if major { th.scale.gamma_multiply(1.6) } else { th.scale };
                self.pt.line_segment([cv.p(x, GY0), cv.p(x, GY1)], cv.stroke(1.0, color));
                if matches!(k, 1 | 2 | 5) && f >= 20.0 {
                    let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
                    self.text(cv.p(x + 3.0, GY1 - 8.0), Align2::LEFT_CENTER, &label, 9.0, th.legend);
                }
            }
            decade *= 10.0;
        }
        let step = RANGE_STEPS[RANGES.iter().position(|r| *r == range).unwrap_or(2)];
        let lines = (range / step).round() as i32;
        for k in -lines..=lines {
            let db = k as f32 * step;
            let y = y_of(db, range);
            let color = if k == 0 { th.scale.gamma_multiply(2.2) } else { th.scale };
            self.pt.line_segment([cv.p(GX0, y), cv.p(GX1, y)], cv.stroke(1.0, color));
            let label = if k == 0 { "0".to_owned() } else { format!("{db:+.0}") };
            self.text(cv.p(GX1 - 4.0, y - 7.0), Align2::RIGHT_CENTER, &label, 9.0, th.legend);
        }
    }

    fn analyzer(&mut self, dt: f32) {
        let mode = self.p.analyzer.load(Relaxed);
        if mode == 0 {
            return;
        }
        let cv = self.cv;
        let sr = self.st.shared.sample_rate.load(Relaxed);
        let shared = self.st.shared.clone();
        let painter = self.pt.with_clip_rect(cv.r(GX0, GY0, GX1 - GX0, GY1 - GY0));
        // Pre in mode 1 and 3, post in 2 and 3.
        for (k, show) in [(0usize, mode & 1 != 0), (1, mode & 2 != 0)] {
            if !show {
                continue;
            }
            let ring = if k == 0 { &shared.pre } else { &shared.post };
            let gs = &mut self.st.graph;
            gs.analyzer.analyze(ring, sr, &gs.freqs, &mut gs.fresh);
            spectrum::smooth(&mut gs.spectrum[k], &gs.fresh, dt);
            if gs.spectrum[k].iter().all(|v| *v <= ANA_BOTTOM) {
                continue;
            }
            let pts: Vec<Pos2> = gs
                .freqs
                .iter()
                .zip(&gs.spectrum[k])
                .map(|(&f, &db)| cv.p(x_of(f), ana_y(db.max(ANA_BOTTOM - 2.0))))
                .collect();
            let color = if k == 0 { self.th.spectrum_pre } else { self.th.spectrum_post };
            if k == 1 || mode == 1 {
                fill_to(&painter, &pts, cv.p(0.0, GY1).y, color.gamma_multiply(0.22));
            }
            painter.add(Shape::line(pts, Stroke::new(1.0 * cv.s, color.gamma_multiply(0.8))));
        }
    }

    /// Where a band's node sits: at its gain, or on the 0 dB line for types
    /// without gain.
    fn node_pos(band: &Band, range: f32) -> Pos2 {
        let y = if band.kind.has_gain() { y_of(band.gain, range) } else { y_of(0.0, range) };
        pos2(x_of(band.freq), y)
    }

    fn nodes(&mut self, bands: &[Band], range: f32) {
        let cv = self.cv;
        let s = cv.s;
        let area = cv.r(GX0, GY0, GX1 - GX0, GY1 - GY0);
        let bg = self.ui.interact(area, Id::new("graph"), Sense::click_and_drag());
        self.register("graph", ControlKind::Graph, area);

        // Draw the selected node last so it sits on top.
        let owners = self.p.owners();
        let mut order: Vec<usize> = (0..NUM_BANDS).filter(|&b| bands[b].on).collect();
        order.sort_by_key(|&b| b == self.st.selected);
        let mut readout = None;
        for &b in &order {
            let band = bands[b];
            let at = Self::node_pos(&band, range);
            let rect = cv.rc(at.x, at.y, NODE_R * 2.0 + 4.0, NODE_R * 2.0 + 4.0);
            let key = format!("node_{}", b + 1);
            let resp = self.ui.interact(rect, Id::new(&key), Sense::click_and_drag());
            self.register(&key, ControlKind::Node, rect);
            let (freq, gain, q) = (Pid::Band(b, Field::Freq), Pid::Band(b, Field::Gain), Pid::Band(b, Field::Q));

            if resp.is_pointer_button_down_on() {
                self.st.selected = b;
            }
            if resp.drag_started() {
                let with_gain = band.kind.has_gain();
                self.begin(freq);
                if with_gain {
                    self.begin(gain);
                }
                self.st.graph.drag = Some(NodeDrag { band: b, start: at, gain: with_gain });
            }
            if let Some(d) = self.st.graph.drag.filter(|d| d.band == b) {
                if resp.dragged() {
                    let p = d.start + self.drag_offset() / s;
                    self.send(freq, freq_at(p.x).clamp(MIN_FREQ, MAX_FREQ));
                    if d.gain {
                        self.send(gain, db_at(p.y, range).clamp(-MAX_GAIN, MAX_GAIN));
                    }
                }
                if resp.drag_stopped() {
                    self.end(freq);
                    if d.gain {
                        self.end(gain);
                    }
                    self.st.graph.drag = None;
                }
            }
            if resp.double_clicked() {
                self.change(Pid::Band(b, Field::On), 0.0);
            }
            if resp.secondary_clicked() {
                let p = resp.interact_pointer_pos().map_or(at, |p| cv.to_canvas(p));
                self.st.selected = b;
                self.st.popup = Some((b, popup_origin(p)));
            }
            if resp.hovered() && !resp.dragged() {
                let wheel = self.wheel();
                if wheel != 0.0 {
                    let fine = if self.ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
                    let v = (band.q * 2f32.powf(wheel.signum() * fine / 6.0)).clamp(MIN_Q, MAX_Q);
                    self.change(q, v);
                }
            }

            // Draw with the values just sent, so the node follows the mouse.
            let band = self.band(b);
            let at = cv.p(Self::node_pos(&band, range).x, Self::node_pos(&band, range).y);
            let color = self.th.band(b);
            let selected = b == self.st.selected;
            let hot = resp.hovered() || resp.dragged();
            self.pt.circle_filled(at + vec2(1.0, 2.0) * s, NODE_R * s, Color32::from_black_alpha(120));
            self.pt.circle_filled(at, NODE_R * s, if selected || hot { color } else { color.gamma_multiply(0.75) });
            if selected {
                self.pt.circle_stroke(at, (NODE_R + 2.5) * s, Stroke::new(1.5 * s, self.th.drag_line));
            }
            self.text(at, Align2::CENTER_CENTER, &(b + 1).to_string(), 10.0, self.th.bar_text);
            if let Some(f) = Feature::from_tag(owners[b]) {
                let dot = at + vec2(-NODE_R, -NODE_R) * s;
                self.pt.circle_filled(dot, 3.0 * s, self.feature_color(f));
                self.pt.circle_stroke(dot, 3.0 * s, Stroke::new(0.8 * s, self.th.display_bg));
            }
            let tag = place_tag(band.place);
            if !tag.is_empty() {
                self.text(at + vec2(NODE_R + 3.0, -NODE_R) * s, Align2::LEFT_CENTER, tag, 9.0, color);
            }
            if hot {
                readout = Some((b, band, at, Feature::from_tag(owners[b])));
            }
        }

        if bg.double_clicked() {
            if let Some(p) = bg.interact_pointer_pos() {
                self.add_band(cv.to_canvas(p), range);
            }
        }

        if let Some((b, band, at, owner)) = readout {
            self.node_readout(b, &band, at, owner);
        } else if let Some(p) = bg.hover_pos() {
            let c = cv.to_canvas(p);
            let text = format!("{}   {}", format_freq(freq_at(c.x)), format_db(db_at(c.y, range)));
            self.text(cv.p(GX0 + 10.0, GY0 + 12.0), Align2::LEFT_CENTER, &text, 9.5, self.th.legend);
        }
    }

    fn node_readout(&mut self, b: usize, band: &Band, at: Pos2, owner: Option<Feature>) {
        let cv = self.cv;
        let mut parts = vec![format!("{}", b + 1), band.kind.name().to_uppercase(), format_freq(band.freq)];
        if band.kind.has_gain() {
            parts.push(format_db(band.gain));
        }
        parts.push(format!("Q {:.2}", band.q));
        if band.kind.has_slope() {
            parts.push(Slope::NAMES[band.slope as usize].to_owned());
        }
        match owner {
            Some(Feature::Resonances) => parts.push("AUTO RES".to_owned()),
            Some(Feature::Balance) => parts.push("AUTO BAL".to_owned()),
            None => {}
        }
        let text = parts.join("  ");
        let w = self.text_w(&text, 9.5) + 16.0;
        let c = cv.to_canvas(at);
        let x = (c.x - w * 0.5).clamp(GX0 + 4.0, GX1 - w - 4.0);
        let y = if c.y - 40.0 > GY0 { c.y - 36.0 } else { c.y + 18.0 };
        let rect = cv.r(x, y, w, 18.0);
        self.pt.rect_filled(rect, cv.round(3.0), self.th.cap);
        self.pt.rect_stroke(rect, cv.round(3.0), cv.stroke(1.0, self.th.band(b)), StrokeKind::Inside);
        self.text(rect.center(), Align2::CENTER_CENTER, &text, 9.5, self.th.text);
    }

    /// Turns on the first free band at canvas point `p`: a low cut near the
    /// left edge, a high cut near the right one, a bell elsewhere.
    pub(crate) fn add_band(&mut self, p: Pos2, range: f32) {
        let Some(b) = (0..NUM_BANDS).find(|&b| self.plain(Pid::Band(b, Field::On)) < 0.5) else {
            return;
        };
        // A band added by hand belongs to no automatic EQ.
        let mut owners = self.p.owners();
        owners[b] = 0;
        self.p.set_owners(owners);
        let freq = freq_at(p.x);
        let (kind, gain, q) = if freq < 25.0 {
            (FilterType::LowCut, 0.0, FLAT_Q)
        } else if freq > 16_000.0 {
            (FilterType::HighCut, 0.0, FLAT_Q)
        } else {
            (FilterType::Bell, db_at(p.y, range).clamp(-MAX_GAIN, MAX_GAIN), 1.0)
        };
        self.change(Pid::Band(b, Field::Kind), kind as i32 as f32);
        self.change(Pid::Band(b, Field::Freq), freq);
        self.change(Pid::Band(b, Field::Gain), gain);
        self.change(Pid::Band(b, Field::Q), q);
        self.change(Pid::Band(b, Field::On), 1.0);
        self.st.selected = b;
    }

    fn meter(&mut self, dt: f32) {
        let cv = self.cv;
        let th = self.th;
        let peaks = self.st.shared.take_peaks();
        let x0 = GX1 + 14.0;
        let y_of_db = |db: f32| GY0 + (METER_TOP - db) / (METER_TOP - METER_BOTTOM) * (GY1 - GY0);
        for (ch, peak) in peaks.iter().enumerate() {
            let db = 20.0 * peak.max(1e-6).log10();
            let shown = &mut self.st.graph.meter[ch];
            *shown = db.max(*shown - 24.0 * dt).max(METER_BOTTOM);
            let x = x0 + ch as f32 * 13.0;
            let slot = cv.r(x, GY0, 10.0, GY1 - GY0);
            self.pt.rect_filled(slot, cv.round(2.0), th.slot);
            let level = *shown;
            if level > METER_BOTTOM {
                let top = y_of_db(level.min(METER_TOP));
                let zero = y_of_db(0.0);
                self.pt.rect_filled(
                    Rect::from_min_max(cv.p(x + 1.0, top.max(zero)), cv.p(x + 9.0, GY1 - 1.0)),
                    cv.round(1.0),
                    th.led,
                );
                if level > 0.0 {
                    self.pt.rect_filled(Rect::from_min_max(cv.p(x + 1.0, top), cv.p(x + 9.0, zero)), cv.round(1.0), th.clip);
                }
            }
        }
        for db in [0.0, -12.0, -24.0, -48.0] {
            let y = y_of_db(db);
            self.pt.line_segment([cv.p(x0 - 3.0, y), cv.p(x0, y)], cv.stroke(1.0, th.legend));
            self.text(cv.p(x0 + 30.0, y), Align2::LEFT_CENTER, &format!("{}", db as i32), 9.0, th.legend);
        }
    }
}
