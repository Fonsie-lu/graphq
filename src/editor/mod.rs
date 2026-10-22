//! Editor in the hardware look of the sibling plugins: coloured section bars,
//! raised buttons, knobs, a powder-coat panel and four colour themes.
//!
//! Layout: a header (analyser mode, graph range, themes), the graph with the
//! band nodes, the spectrum and the output meter, and below it the band strip
//! (band chips, type, frequency, gain, Q, slope, placement), the automatic EQ
//! buttons and the output gain.
//!
//! Everything is painted on a fixed `BW` x `BH` canvas through [`Cv`] and
//! scaled uniformly to the window; drag the bottom right corner to resize.

mod graph;
pub mod theme;
mod widgets;

use nih_plug::prelude::*;
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{self, pos2, vec2, Align2, Color32, FontFamily, Id, Pos2, Rect, Sense, TextureHandle, Ui};
use nih_plug_egui::resizable_window::ResizableWindow;
use std::collections::HashMap;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use crate::dsp::auto::{self, Feature, CAPTURE_SECONDS};
use crate::dsp::eq::NUM_BANDS;
use crate::dsp::filter::{Band, FilterType, Placement, Slope, FLAT_Q};
use crate::dsp::shared::Shared;
use crate::params::{Field, GraphqParams};
use theme::{Section, Theme, THEMES};
pub use widgets::{Cv, Pid, G};

/// Canvas size in logical pixels.
pub(crate) const BW: f32 = 1200.0;
pub(crate) const BH: f32 = 640.0;
/// Default window size: the canvas at 100 %.
pub const WIDTH: u32 = BW as u32;
pub const HEIGHT: u32 = BH as u32;

/// Left edge and width of the AUTO EQ section.
const AUTO_X: f32 = 936.0;
const BW_AUTO: f32 = 176.0;

/// How long a value set by the GUI is shown before trusting the parameter again.
pub(crate) const PENDING_TIMEOUT: f64 = 1.0;

/// Analyser modes, indexed by `GraphqParams::analyzer`.
pub const ANALYZER_MODES: [&str; 4] = ["OFF", "PRE", "POST", "PRE+POST"];
pub const DEFAULT_ANALYZER: u8 = 3;
/// Graph ranges in ± dB, indexed by `GraphqParams::range`, and their grid steps.
pub const RANGES: [f32; 5] = [3.0, 6.0, 12.0, 24.0, 30.0];
pub const RANGE_STEPS: [f32; 5] = [1.0, 2.0, 3.0, 6.0, 10.0];
pub const DEFAULT_RANGE: u8 = 2;

/// Short names for the type cells, in `FilterType` order.
const TYPE_LABELS: [&str; 8] = ["BELL", "LO SHELF", "HI SHELF", "LO CUT", "HI CUT", "NOTCH", "BAND PASS", "TILT"];
const SLOPE_LABELS: [&str; 6] = ["6", "12", "18", "24", "36", "48"];
const PLACE_LABELS: [&str; 5] = ["L+R", "L", "R", "MID", "SIDE"];

/// Shared parameter store used by the standalone preview and the tests,
/// which have no plugin wrapper that could apply parameter changes.
pub type PreviewStore = Arc<Mutex<HashMap<ParamPtr, f32>>>;

pub(crate) struct Pending {
    pub value: f32,
    pub time: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlKind {
    Knob,
    /// Choice as cells; registered whole under the parameter ID and per cell
    /// as `"<id>#<index>"`.
    List,
    Button,
    /// Band node on the graph, registered as `"node_<n>"`.
    Node,
    Graph,
}

/// Screen rectangle of an interactive control, recorded every frame (tests).
#[derive(Clone, Debug)]
pub struct ControlInfo {
    pub name: String,
    pub kind: ControlKind,
    pub rect: Rect,
}

pub struct EditorState {
    pub(crate) shared: Arc<Shared>,
    /// Band shown in the strip and highlighted on the graph.
    pub selected: usize,
    pub(crate) pending: HashMap<Pid, Pending>,
    pub(crate) preview_store: Option<PreviewStore>,
    pub controls: Vec<ControlInfo>,
    /// Type menu of a band, opened by right-clicking its node (canvas position).
    pub(crate) popup: Option<(usize, Pos2)>,
    /// Knob travel position when its drag started.
    pub(crate) knob_start: f32,
    pub(crate) graph: graph::GraphState,
    texture: Option<(u8, TextureHandle)>,
}

impl EditorState {
    pub fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            selected: 0,
            pending: HashMap::new(),
            preview_store: None,
            controls: Vec::new(),
            popup: None,
            knob_start: 0.0,
            graph: graph::GraphState::default(),
            texture: None,
        }
    }

    /// Read parameter values from `store` instead of the parameters themselves.
    pub fn with_preview_store(mut self, store: PreviewStore) -> Self {
        self.preview_store = Some(store);
        self
    }
}

pub fn create(params: Arc<GraphqParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        EditorState::new(shared),
        |ctx, _| setup_style(ctx),
        move |ctx, setter, state| root(ctx, &params, setter, state),
    )
}

pub fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "panel".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/DejaVuSansCondensed-Bold.ttf"))),
    );
    fonts.font_data.insert(
        "oled".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/DejaVuSansMono-Bold.ttf"))),
    );
    fonts.families.insert(FontFamily::Name("panel".into()), vec!["panel".to_owned()]);
    fonts.families.insert(FontFamily::Name("oled".into()), vec!["oled".to_owned()]);
    ctx.set_fonts(fonts);
}

pub fn root(ctx: &egui::Context, params: &GraphqParams, setter: &ParamSetter, state: &mut EditorState) {
    // The analyser and the meter animate without any input.
    ctx.request_repaint();
    ResizableWindow::new("graphq")
        .min_size(vec2(BW * 0.5, BH * 0.5))
        .show(ctx, params.editor_state.as_ref(), |ui| draw(ui, params, setter, state));
}

fn draw(ui: &mut Ui, p: &GraphqParams, set: &ParamSetter, st: &mut EditorState) {
    let full = ui.max_rect();
    let s = (full.width() / BW).min(full.height() / BH);
    let o = pos2(full.center().x - BW * s * 0.5, full.center().y - BH * s * 0.5);
    let cv = Cv { o, s };

    let theme_index = p.theme.load(Relaxed);
    let th = theme::get(theme_index);
    if st.texture.as_ref().map(|(i, _)| *i) != Some(theme_index) {
        let tex = ui.ctx().load_texture("panel-texture", panel_texture(th), egui::TextureOptions::LINEAR_REPEAT);
        st.texture = Some((theme_index, tex));
    }
    if ui.style().visuals.panel_fill != th.outer {
        ui.ctx().style_mut(|s| s.visuals.panel_fill = th.outer);
    }
    st.controls.clear();

    let pt = ui.painter().clone();
    let mut g = G { ui, pt, cv, set, p, st, th };
    g.panel();
    g.header();
    // A popup takes the clicks: controls underneath must not react to the
    // click that is meant for a menu entry.
    if let Some((_, at)) = g.st.popup {
        g.ui.interact(popup_rect(cv, at), Id::new("popup-shield"), Sense::click_and_drag());
    }
    g.bar(46.0, &[(12.0, BW - 12.0, "EQUALIZER", Section::Graph)]);
    g.graph();
    g.bar(
        470.0,
        &[
            (12.0, AUTO_X, "BAND", Section::Band),
            (AUTO_X, AUTO_X + BW_AUTO, "AUTO EQ", Section::Auto),
            (AUTO_X + BW_AUTO, BW - 12.0, "OUTPUT", Section::Output),
        ],
    );
    g.band_strip();
    g.auto_section();
    g.output_section();
    g.popup();
}

fn panel_texture(th: &Theme) -> egui::ColorImage {
    let n = 256;
    // Small LCG: the texture only needs to look random and stay the same.
    let mut seed = 77u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut px = Vec::with_capacity(n * n);
    for _ in 0..n * n {
        let grain = (next() * 2.0 - 1.0) * th.grain;
        let fleck = if next() > 0.985 { th.fleck } else { 0.0 };
        let v = |c: u8| (c as f32 + grain + fleck).clamp(0.0, 255.0) as u8;
        px.push(Color32::from_rgb(v(th.panel.r()), v(th.panel.g()), v(th.panel.b())));
    }
    egui::ColorImage { size: [n, n], pixels: px }
}

const POPUP_W: f32 = 120.0;
const POPUP_ROW: f32 = 20.0;

fn popup_rect(cv: Cv, at: Pos2) -> Rect {
    cv.r(at.x, at.y, POPUP_W, POPUP_ROW * (TYPE_LABELS.len() + 1) as f32 + 6.0)
}

/// Popup origin for a menu opened at `p` (canvas), kept inside the canvas.
pub(crate) fn popup_origin(p: Pos2) -> Pos2 {
    let h = POPUP_ROW * (TYPE_LABELS.len() + 1) as f32 + 6.0;
    pos2(p.x.min(BW - POPUP_W - 4.0), p.y.min(BH - h - 4.0))
}

impl G<'_, '_> {
    fn panel(&self) {
        let cv = self.cv;
        let rect = cv.r(0.0, 0.0, BW, BH);
        let tex = &self.st.texture.as_ref().unwrap().1;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / (256.0 * cv.s), rect.height() / (256.0 * cv.s)));
        self.pt.image(tex.id(), rect, uv, Color32::WHITE);
        self.pt.line_segment([cv.p(2.0, 1.0), cv.p(BW - 2.0, 1.0)], cv.stroke(1.5, self.th.bevel));
    }

    fn header(&mut self) {
        let cv = self.cv;
        self.text(cv.p(18.0, 23.0), Align2::LEFT_CENTER, "GRAPHQ", 17.0, self.th.text);
        self.text(cv.p(98.0, 24.0), Align2::LEFT_CENTER, "PARAMETRIC EQUALIZER", 9.0, self.th.header);

        let mut x = 250.0;
        self.text(cv.p(x, 23.0), Align2::LEFT_CENTER, "ANALYZER", 9.0, self.th.header);
        x += 62.0;
        let mode = self.p.analyzer.load(Relaxed) as usize;
        for (i, name) in ANALYZER_MODES.iter().enumerate() {
            let (resp, w) = self.chip(&format!("analyzer:{name}"), x, 15.0, name, i == mode);
            if resp.clicked() {
                self.p.analyzer.store(i as u8, Relaxed);
            }
            x += w + 4.0;
        }

        x += 16.0;
        self.text(cv.p(x, 23.0), Align2::LEFT_CENTER, "RANGE", 9.0, self.th.header);
        x += 42.0;
        let range = self.p.range.load(Relaxed) as usize;
        for (i, r) in RANGES.iter().enumerate() {
            let (resp, w) = self.chip(&format!("range:{r}"), x, 15.0, &format!("±{r:.0}"), i == range);
            if resp.clicked() {
                self.p.range.store(i as u8, Relaxed);
            }
            x += w + 4.0;
        }

        let current = self.p.theme.load(Relaxed) as usize;
        let widths: Vec<f32> = THEMES.iter().map(|t| 14.0 + self.text_w(t.name, 9.0)).collect();
        let total: f32 = widths.iter().map(|w| w + 4.0).sum::<f32>() - 4.0;
        let mut x = BW - 14.0 - total;
        for (i, t) in THEMES.iter().enumerate() {
            let (resp, w) = self.chip(&format!("theme:{}", t.name), x, 15.0, t.name, i == current);
            if resp.clicked() {
                self.p.theme.store(i as u8, Relaxed);
            }
            x += w + 4.0;
        }
    }

    fn band_strip(&mut self) {
        let cv = self.cv;
        let sel = self.st.selected;
        let owners = self.p.owners();

        // Band chips: number, coloured when the band is on, a dot in the
        // colour of the automatic EQ that made it.
        self.caption(22.0, 510.0, "BANDS");
        for (b, &owner) in owners.iter().enumerate() {
            let x = 22.0 + (b % 8) as f32 * 26.0;
            let y = 514.0 + (b / 8) as f32 * 30.0;
            let rect = cv.r(x, y, 23.0, 24.0);
            let key = format!("band:{}", b + 1);
            let resp = self.ui.interact(rect, Id::new(&key), Sense::click());
            self.register(&key, ControlKind::Button, rect);
            let on = self.plain(Pid::Band(b, Field::On)) > 0.5;
            let color = self.th.band(b);
            let face = if on {
                color
            } else if resp.hovered() {
                self.th.button_hover
            } else {
                self.th.button
            };
            self.pt.rect_filled(rect.translate(vec2(0.0, 2.0 * cv.s)), cv.round(3.0), Color32::from_black_alpha(140));
            self.pt.rect_filled(rect, cv.round(3.0), face);
            if b == sel {
                self.pt.rect_stroke(rect.expand(2.0 * cv.s), cv.round(4.0), cv.stroke(1.5, self.th.drag_line), egui::StrokeKind::Outside);
            }
            let text_ink = if on { self.th.bar_text } else { color };
            self.text(rect.center(), Align2::CENTER_CENTER, &(b + 1).to_string(), 10.5, text_ink);
            if let Some(f) = Feature::from_tag(owner) {
                let dot = rect.right_top() + vec2(-4.0, 4.0) * cv.s;
                self.pt.circle_filled(dot, 3.0 * cv.s, self.feature_color(f));
                self.pt.circle_stroke(dot, 3.0 * cv.s, cv.stroke(0.8, self.th.slot));
            }
            if resp.clicked() {
                self.st.selected = b;
            }
            if resp.double_clicked() {
                self.change(Pid::Band(b, Field::On), if on { 0.0 } else { 1.0 });
            }
        }

        // A chip clicked this frame already shows its band.
        let sel = self.st.selected;
        let ink = self.th.band(sel);
        let band = self.band(sel);
        let on_id = Pid::Band(sel, Field::On);
        self.caption(240.0, 510.0, &format!("BAND {}", sel + 1));
        if self.button(&on_id.id(), 240.0, 514.0, 50.0, 24.0, if band.on { "ON" } else { "OFF" }, band.on, ink, true) {
            self.change(on_id, if band.on { 0.0 } else { 1.0 });
        }
        self.led(cv.p(265.0, 556.0), band.on, ink);
        if let Some(f) = Feature::from_tag(owners[sel]) {
            let tag = match f {
                Feature::Resonances => "AUTO RES",
                Feature::Balance => "AUTO BAL",
            };
            self.text(cv.p(265.0, 578.0), Align2::CENTER_CENTER, tag, 9.0, self.feature_color(f));
        }

        self.caption(304.0, 510.0, "TYPE");
        if let Some(t) = self.segments(Pid::Band(sel, Field::Kind), 304.0, 514.0, 284.0, 26.0, 4, &TYPE_LABELS, ink, true) {
            self.adapt_q(sel, FilterType::ALL[t]);
        }

        let kind = band.kind;
        self.knob(Pid::Band(sel, Field::Freq), 632.0, 556.0, 24.0, "FREQ", ink, true);
        self.knob(Pid::Band(sel, Field::Gain), 704.0, 556.0, 24.0, "GAIN", ink, kind.has_gain());
        // A 6 dB/oct cut has no pole pair for Q to shape.
        let q_used = !(kind.has_slope() && band.slope == Slope::Db6);
        self.knob(Pid::Band(sel, Field::Q), 776.0, 556.0, 24.0, "Q", ink, q_used);

        self.caption(822.0, 510.0, "SLOPE dB/oct");
        self.segments(Pid::Band(sel, Field::Slope), 822.0, 514.0, 104.0, 20.0, 3, &SLOPE_LABELS, ink, kind.has_slope());
        self.caption(822.0, 574.0, "PLACE");
        self.segments(Pid::Band(sel, Field::Place), 822.0, 578.0, 104.0, 20.0, 3, &PLACE_LABELS, ink, true);
    }

    pub(crate) fn feature_color(&self, f: Feature) -> Color32 {
        match f {
            Feature::Resonances => self.th.sublabel,
            Feature::Balance => self.th.accent,
        }
    }

    /// The two automatic EQ buttons. A click records the next
    /// [`CAPTURE_SECONDS`] of input; when the capture is full the analysis
    /// runs here and its bands replace the ones the same feature made before.
    /// One run at a time; clicking the running one cancels it.
    fn auto_section(&mut self) {
        let cv = self.cv;
        let shared = self.st.shared.clone();
        let finished = shared.poll_run(|feature| {
            let owners = self.p.owners();
            (0..NUM_BANDS)
                .filter(|&b| owners[b] != feature as u8)
                .map(|b| self.band(b))
                .filter(|b| b.on)
                .collect()
        });
        if let Some((feature, result)) = finished {
            let placed = match &result {
                Ok(bands) => self.apply_auto(feature, bands),
                Err(_) => 0,
            };
            shared.finish_run(auto::status(feature, &result, placed));
        }
        let (running, progress, status) = shared.run_state();

        let x = AUTO_X + 10.0;
        let w = BW_AUTO - 20.0;
        for (feature, key, label, y) in [
            (Feature::Resonances, "auto:resonances", "1  RESONANCES", 512.0),
            (Feature::Balance, "auto:balance", "2  BALANCE (MASTER)", 546.0),
        ] {
            let active = running == Some(feature);
            let enabled = running.is_none() || active;
            let color = self.feature_color(feature);
            if self.button(key, x, y, w, 26.0, label, active, color, enabled) {
                if active {
                    shared.cancel_run();
                } else {
                    shared.begin_run(feature);
                }
            }
            self.pt.circle_filled(cv.p(x + 10.0, y + 13.0), 3.0 * cv.s, if enabled { color } else { self.th.legend });
        }

        let bar = cv.r(x, 582.0, w, 6.0);
        self.pt.rect_filled(bar, cv.round(2.0), self.th.slot);
        let (status, color) = match (running, progress) {
            (Some(f), None) => ("ANALYZING".to_owned(), self.feature_color(f)),
            (Some(f), Some(p)) => {
                let fill = Rect::from_min_size(bar.min, vec2(bar.width() * p, bar.height()));
                self.pt.rect_filled(fill, cv.round(2.0), self.feature_color(f));
                (format!("RECORDING  {:.1} / {CAPTURE_SECONDS:.0} s", p * CAPTURE_SECONDS), self.feature_color(f))
            }
            _ => (status, self.th.text),
        };
        self.text(cv.p(x + w * 0.5, 602.0), Align2::CENTER_CENTER, &status, 9.0, color);
        self.text(cv.p(x + w * 0.5, 620.0), Align2::CENTER_CENTER, "1 THEN 2 · CUTS ONLY", 9.0, self.th.legend);
    }

    /// Writes the bands of a finished run as [`auto::plan`] assigns them.
    /// Returns how many bands were placed.
    pub(crate) fn apply_auto(&mut self, feature: Feature, bands: &[Band]) -> usize {
        let on: Vec<bool> = (0..NUM_BANDS).map(|b| self.plain(Pid::Band(b, Field::On)) > 0.5).collect();
        let plan = auto::plan(feature, &on, &self.p.owners(), bands);
        for &(b, band) in &plan.writes {
            self.change(Pid::Band(b, Field::Kind), band.kind as i32 as f32);
            self.change(Pid::Band(b, Field::Freq), band.freq);
            self.change(Pid::Band(b, Field::Gain), band.gain);
            self.change(Pid::Band(b, Field::Q), band.q);
            self.change(Pid::Band(b, Field::Slope), band.slope as i32 as f32);
            self.change(Pid::Band(b, Field::Place), band.place as i32 as f32);
            self.change(Pid::Band(b, Field::On), 1.0);
        }
        for &b in &plan.off {
            self.change(Pid::Band(b, Field::On), 0.0);
        }
        self.p.set_owners(std::array::from_fn(|b| plan.owners[b]));
        plan.writes.len()
    }

    /// Picks a sensible Q when the type changes and Q is still at the other
    /// family's neutral value: 0.71 for cuts and shelves, 1 for the rest.
    pub(crate) fn adapt_q(&mut self, band: usize, kind: FilterType) {
        let id = Pid::Band(band, Field::Q);
        let q = self.plain(id);
        let flat_family = matches!(
            kind,
            FilterType::LowCut | FilterType::HighCut | FilterType::LowShelf | FilterType::HighShelf | FilterType::TiltShelf
        );
        if flat_family && (q - 1.0).abs() < 1e-3 {
            self.change(id, FLAT_Q);
        } else if !flat_family && (q - FLAT_Q).abs() < 1e-3 {
            self.change(id, 1.0);
        }
    }

    fn output_section(&mut self) {
        let accent = self.th.bars[Section::Output as usize];
        let cx = (AUTO_X + BW_AUTO + BW - 12.0) * 0.5;
        self.knob(Pid::Output, cx, 556.0, 24.0, "OUTPUT", accent, true);
        let active = (0..NUM_BANDS).filter(|&b| self.plain(Pid::Band(b, Field::On)) > 0.5).count();
        self.text(self.cv.p(cx, 620.0), Align2::CENTER_CENTER, &format!("{active}/{NUM_BANDS} BANDS"), 9.0, self.th.legend);
    }

    fn popup(&mut self) {
        let Some((band, at)) = self.st.popup else { return };
        let cv = self.cv;
        let rect = popup_rect(cv, at);
        self.pt.rect_filled(rect.translate(vec2(0.0, 3.0 * cv.s)), cv.round(4.0), Color32::from_black_alpha(160));
        self.pt.rect_filled(rect, cv.round(4.0), self.th.cap);
        self.pt.rect_stroke(rect, cv.round(4.0), cv.stroke(1.0, self.th.band(band)), egui::StrokeKind::Inside);
        let current = self.plain(Pid::Band(band, Field::Kind)).round() as usize;
        let mut close = false;
        let labels = FilterType::NAMES.iter().map(|n| n.to_uppercase()).chain(["REMOVE".to_owned()]);
        for (i, label) in labels.enumerate() {
            let r = cv.r(at.x + 3.0, at.y + 3.0 + i as f32 * POPUP_ROW, POPUP_W - 6.0, POPUP_ROW);
            let key = format!("popup:{i}");
            let resp = self.ui.interact(r, Id::new(&key), Sense::click());
            self.register(&key, ControlKind::Button, r);
            if resp.hovered() {
                self.pt.rect_filled(r, cv.round(2.0), self.th.button_hover);
            }
            let ink = if i == current { self.th.band(band) } else { self.th.text };
            self.text(r.left_center() + vec2(8.0 * cv.s, 0.0), Align2::LEFT_CENTER, &label, 9.5, ink);
            if resp.clicked() {
                if i < TYPE_LABELS.len() {
                    if i != current {
                        self.change(Pid::Band(band, Field::Kind), i as f32);
                        self.adapt_q(band, FilterType::ALL[i]);
                    }
                } else {
                    self.change(Pid::Band(band, Field::On), 0.0);
                }
                close = true;
            }
        }
        let outside = self.ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        });
        if close || outside || self.ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.st.popup = None;
        }
    }
}

/// Labels for the stereo placement, shown on the nodes.
pub(crate) fn place_tag(place: Placement) -> &'static str {
    match place {
        Placement::Stereo => "",
        Placement::Left => "L",
        Placement::Right => "R",
        Placement::Mid => "M",
        Placement::Side => "S",
    }
}

#[cfg(test)]
#[path = "../editor_tests.rs"]
mod tests;
