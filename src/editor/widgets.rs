//! Canvas mapping, parameter I/O and the basic controls: knobs, choice cells,
//! buttons and chips, in the hardware look of the sibling plugins.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Mesh, Painter, Pos2, Rect,
    Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
};
use std::f32::consts::PI;

use super::theme::{Section, Theme};
use super::{ControlInfo, ControlKind, EditorState, Pending, PENDING_TIMEOUT};
use crate::dsp::filter::{Band, FilterType, Placement, Slope};
use crate::params::{Field, GraphqParams};

/// Smallest text size in canvas pixels.
pub const MIN_FONT: f32 = 10.0;
/// Vertical drag across a knob's full range, in canvas pixels.
pub const KNOB_TRAVEL: f32 = 200.0;

/// Maps canvas coordinates to screen coordinates.
#[derive(Clone, Copy)]
pub struct Cv {
    pub o: Pos2,
    pub s: f32,
}

impl Cv {
    pub fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.o.x + x * self.s, self.o.y + y * self.s)
    }
    pub fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), vec2(w * self.s, h * self.s))
    }
    pub fn rc(&self, cx: f32, cy: f32, w: f32, h: f32) -> Rect {
        self.r(cx - w * 0.5, cy - h * 0.5, w, h)
    }
    pub fn font(&self, size: f32) -> FontId {
        FontId::new(size.max(MIN_FONT) * self.s, FontFamily::Name("panel".into()))
    }
    pub fn mono(&self, size: f32) -> FontId {
        FontId::new(size.max(MIN_FONT) * self.s, FontFamily::Name("oled".into()))
    }
    pub fn stroke(&self, w: f32, c: Color32) -> Stroke {
        Stroke::new(w * self.s, c)
    }
    pub fn round(&self, r: f32) -> CornerRadius {
        CornerRadius::same((r * self.s) as u8)
    }
    /// Screen position back to canvas coordinates.
    pub fn to_canvas(&self, p: Pos2) -> Pos2 {
        pos2((p.x - self.o.x) / self.s, (p.y - self.o.y) / self.s)
    }
}

/// A host parameter as the editor addresses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pid {
    Band(usize, Field),
    Output,
}

impl Pid {
    /// The parameter ID, also the key controls register under.
    pub fn id(self) -> String {
        match self {
            Pid::Band(b, f) => {
                let name = match f {
                    Field::On => "on",
                    Field::Kind => "type",
                    Field::Freq => "freq",
                    Field::Gain => "gain",
                    Field::Q => "q",
                    Field::Slope => "slope",
                    Field::Place => "place",
                };
                format!("{name}_{}", b + 1)
            }
            Pid::Output => "output".to_owned(),
        }
    }

    /// Knob travel: logarithmic for frequency and Q.
    fn log_travel(self) -> bool {
        matches!(self, Pid::Band(_, Field::Freq | Field::Q))
    }
}

pub struct G<'a, 'b> {
    pub ui: &'a Ui,
    pub pt: Painter,
    pub cv: Cv,
    pub set: &'a ParamSetter<'b>,
    pub p: &'a GraphqParams,
    pub st: &'a mut EditorState,
    pub th: &'static Theme,
}

impl G<'_, '_> {
    // ------------------------------------------------------------ text

    pub fn text(&self, pos: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
        self.pt.text(pos, align, text, self.cv.font(size), color);
    }

    /// Width of `text` in the panel font, in canvas pixels.
    pub fn text_w(&self, text: &str, size: f32) -> f32 {
        let font = self.cv.font(size);
        self.ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x) / self.cv.s
    }

    pub fn bar(&self, y: f32, sections: &[(f32, f32, &str, Section)]) {
        let cv = self.cv;
        for (a, b, title, section) in sections {
            let rect = cv.r(a + 1.5, y, b - a - 3.0, 22.0);
            self.pt.rect_filled(rect, cv.round(3.0), self.th.bars[*section as usize]);
            self.text(cv.p((a + b) * 0.5, y + 11.5), Align2::CENTER_CENTER, title, 14.0, self.th.bar_text);
        }
    }

    /// Small caption above a control.
    pub fn caption(&self, x: f32, y: f32, text: &str) {
        self.text(self.cv.p(x, y), Align2::LEFT_BOTTOM, text, 9.0, self.th.legend);
    }

    // ------------------------------------------------------- parameter I/O

    pub fn register(&mut self, name: &str, kind: ControlKind, rect: Rect) {
        self.st.controls.push(ControlInfo { name: name.to_owned(), kind, rect });
    }

    pub fn now(&self) -> f64 {
        self.ui.input(|i| i.time)
    }

    pub fn ptr(&self, id: Pid) -> ParamPtr {
        match id {
            Pid::Band(b, f) => self.p.bands[b].ptr(f),
            Pid::Output => self.p.output.as_ptr(),
        }
    }

    /// The parameter's actual normalised value (from the preview store in the
    /// standalone preview and the tests).
    pub fn actual(&self, id: Pid) -> f32 {
        let ptr = self.ptr(id);
        if let Some(store) = &self.st.preview_store {
            if let Some(v) = store.lock().unwrap().get(&ptr) {
                return *v;
            }
        }
        unsafe { ptr.unmodulated_normalized_value() }
    }

    /// The normalised value to display: a value the GUI just sent wins until
    /// the host has applied it (nih-plug applies GUI changes asynchronously).
    pub fn shown(&mut self, id: Pid) -> f32 {
        let actual = self.actual(id);
        let now = self.now();
        match self.st.pending.get(&id) {
            Some(p) if (p.value - actual).abs() > 1e-6 && now - p.time < PENDING_TIMEOUT => p.value,
            Some(_) => {
                self.st.pending.remove(&id);
                actual
            }
            None => actual,
        }
    }

    /// Displayed value in natural units.
    pub fn plain(&mut self, id: Pid) -> f32 {
        let n = self.shown(id);
        unsafe { self.ptr(id).preview_plain(n) }
    }

    /// A band as displayed.
    pub fn band(&mut self, b: usize) -> Band {
        let mut v = |f| self.plain(Pid::Band(b, f));
        Band {
            on: v(Field::On) > 0.5,
            kind: FilterType::from_index(v(Field::Kind).round() as i32),
            freq: v(Field::Freq),
            gain: v(Field::Gain),
            q: v(Field::Q),
            slope: Slope::from_index(v(Field::Slope).round() as i32),
            place: Placement::from_index(v(Field::Place).round() as i32),
        }
    }

    pub fn value_text(&mut self, id: Pid) -> String {
        let n = self.shown(id);
        unsafe { self.ptr(id).normalized_value_to_string(n, true) }
    }

    /// Sends a value in natural units (snapped by the parameter's range) and
    /// remembers it as pending. Must sit between `begin` and `end`.
    pub fn send(&mut self, id: Pid, plain: f32) {
        let ptr = self.ptr(id);
        let n = unsafe { ptr.preview_normalized(plain) };
        if (n - self.shown(id)).abs() < 1e-7 {
            return;
        }
        unsafe { self.set.raw_context.raw_set_parameter_normalized(ptr, n) };
        let time = self.now();
        self.st.pending.insert(id, Pending { value: n, time });
    }

    pub fn begin(&self, id: Pid) {
        unsafe { self.set.raw_context.raw_begin_set_parameter(self.ptr(id)) };
    }

    pub fn end(&self, id: Pid) {
        unsafe { self.set.raw_context.raw_end_set_parameter(self.ptr(id)) };
    }

    /// A complete single-step change (begin, set, end) in natural units.
    pub fn change(&mut self, id: Pid, plain: f32) {
        self.begin(id);
        self.send(id, plain);
        self.end(id);
    }

    pub fn default_plain(&self, id: Pid) -> f32 {
        unsafe { self.ptr(id).default_plain_value() }
    }

    /// Position on a knob's travel (0 … 1) for a value in natural units.
    fn travel(&self, id: Pid, plain: f32) -> f32 {
        let ptr = self.ptr(id);
        let (lo, hi) = unsafe { (ptr.preview_plain(0.0), ptr.preview_plain(1.0)) };
        if id.log_travel() {
            (plain / lo).ln() / (hi / lo).ln()
        } else {
            (plain - lo) / (hi - lo)
        }
    }

    fn value_at_travel(&self, id: Pid, t: f32) -> f32 {
        let ptr = self.ptr(id);
        let (lo, hi) = unsafe { (ptr.preview_plain(0.0), ptr.preview_plain(1.0)) };
        let t = t.clamp(0.0, 1.0);
        if id.log_travel() {
            lo * (hi / lo).powf(t)
        } else {
            lo + (hi - lo) * t
        }
    }

    /// Pointer movement since the button went down, scaled down with Shift.
    /// Used instead of per-frame deltas so the movement before egui
    /// recognises the drag is not lost.
    pub fn drag_offset(&self) -> Vec2 {
        self.ui.input(|i| {
            let fine = if i.modifiers.shift { 0.15 } else { 1.0 };
            match (i.pointer.press_origin(), i.pointer.interact_pos()) {
                (Some(a), Some(b)) => (b - a) * fine,
                _ => Vec2::ZERO,
            }
        })
    }

    /// Mouse wheel movement over the hovered widget this frame (up positive).
    pub fn wheel(&self) -> f32 {
        self.ui.input(|i| i.raw_scroll_delta.y)
    }

    // ------------------------------------------------------- controls

    /// Knob: drag vertically (Shift: fine), wheel, double-click resets.
    #[allow(clippy::too_many_arguments)]
    pub fn knob(&mut self, id: Pid, cx: f32, cy: f32, r: f32, label: &str, ink: Color32, enabled: bool) {
        let cv = self.cv;
        let s = cv.s;
        let key = id.id();
        let rect = cv.rc(cx, cy + 6.0, 2.0 * r + 24.0, 2.0 * r + 32.0);
        self.register(&key, ControlKind::Knob, rect);
        let resp = self.ui.interact(rect, Id::new(("knob", &key)), Sense::click_and_drag());
        let plain = self.plain(id);
        if enabled {
            if resp.double_clicked() {
                let d = self.default_plain(id);
                self.change(id, d);
            } else if resp.drag_started() {
                self.begin(id);
                self.st.knob_start = self.travel(id, plain);
            } else if resp.dragged() {
                let t = self.st.knob_start - self.drag_offset().y / s / KNOB_TRAVEL;
                let v = self.value_at_travel(id, t);
                self.send(id, v);
            }
            if resp.drag_stopped() {
                self.end(id);
            }
            let wheel = if resp.hovered() { self.wheel() } else { 0.0 };
            if wheel != 0.0 && !resp.dragged() {
                let fine = if self.ui.input(|i| i.modifiers.shift) { 0.2 } else { 1.0 };
                let t = self.travel(id, plain) + wheel.signum() * 0.02 * fine;
                let v = self.value_at_travel(id, t);
                self.change(id, v);
            }
        }
        let active = enabled && (resp.hovered() || resp.dragged());
        let now = self.plain(id);
        let t = self.travel(id, now).clamp(0.0, 1.0);

        let c = cv.p(cx, cy);
        let at = |t: f32, radius: f32| {
            let a = (0.75 + 1.5 * t) * PI;
            c + vec2(a.cos(), a.sin()) * radius * s
        };
        let arc = |a: f32, b: f32, radius: f32, width: f32, color: Color32| {
            let (a, b) = (a.min(b), a.max(b));
            let n = ((b - a) * 48.0).ceil().max(1.0) as usize;
            let pts: Vec<Pos2> = (0..=n).map(|k| at(a + (b - a) * k as f32 / n as f32, radius)).collect();
            self.pt.add(Shape::line(pts, Stroke::new(width * s, color)));
        };
        let origin = if matches!(id, Pid::Band(_, Field::Gain) | Pid::Output) { 0.5 } else { 0.0 };
        arc(0.0, 1.0, r + 4.0, 2.5, self.th.slot);
        let ink = if enabled { ink } else { ink.gamma_multiply(0.3) };
        if (t - origin).abs() > 1e-4 {
            arc(origin, t, r + 4.0, 2.5, ink);
        }
        self.pt.circle_filled(c + vec2(1.5, 3.0) * s, r * s, Color32::from_black_alpha(140));
        self.pt.circle_filled(c, r * s, self.th.cap_bottom);
        self.pt.circle_filled(c - vec2(0.0, 1.0) * s, (r - 2.0) * s, self.th.cap);
        let pointer = if resp.dragged() { self.th.drag_line } else { self.th.text };
        let pointer = if enabled { pointer } else { self.th.legend };
        self.pt.line_segment([at(t, r * 0.3), at(t, r * 0.85)], Stroke::new(2.2 * s, pointer));
        let text = if active { self.value_text(id) } else { label.to_owned() };
        let color = if !enabled {
            self.th.legend
        } else if active {
            ink
        } else {
            self.th.text
        };
        self.text(cv.p(cx, cy + r + 15.0), Align2::CENTER_CENTER, &text, 10.0, color);
        if !active && enabled {
            let value = self.value_text(id);
            self.text(cv.p(cx, cy - r - 12.0), Align2::CENTER_CENTER, &value, 9.5, self.th.legend);
        }
    }

    /// Choice parameter as a grid of cells, `cols` per row in value order.
    /// Returns the value clicked this frame.
    #[allow(clippy::too_many_arguments)]
    pub fn segments(
        &mut self,
        id: Pid,
        x: f32,
        y: f32,
        w: f32,
        cell_h: f32,
        cols: usize,
        labels: &[&str],
        ink: Color32,
        enabled: bool,
    ) -> Option<usize> {
        let cv = self.cv;
        let count = labels.len();
        let cell_w = w / cols as f32;
        let key = id.id();
        let whole = cv.r(x, y, w, count.div_ceil(cols) as f32 * cell_h);
        self.register(&key, ControlKind::List, whole);
        let current = self.plain(id).round() as usize;
        let mut clicked = None;
        for (i, label) in labels.iter().enumerate() {
            let rect = cv
                .r(x + (i % cols) as f32 * cell_w, y + (i / cols) as f32 * cell_h, cell_w, cell_h)
                .shrink(1.0 * cv.s);
            let cell_key = format!("{key}#{i}");
            let resp = self.ui.interact(rect, Id::new(&cell_key), Sense::click());
            self.register(&cell_key, ControlKind::Button, rect);
            let selected = i == current;
            let face = if selected {
                self.th.slot
            } else if resp.hovered() && enabled {
                self.th.button_hover
            } else {
                self.th.button
            };
            self.pt.rect_filled(rect, cv.round(2.0), face);
            if selected {
                let edge = if enabled { ink } else { self.th.legend };
                self.pt.rect_stroke(rect, cv.round(2.0), Stroke::new(1.0 * cv.s, edge), StrokeKind::Inside);
            }
            let text_ink = match (enabled, selected) {
                (false, _) => self.th.legend,
                (true, true) => ink,
                (true, false) => self.th.text,
            };
            self.text(rect.center(), Align2::CENTER_CENTER, label, 9.0, text_ink);
            if enabled && resp.clicked() && !selected {
                self.change(id, i as f32);
                clicked = Some(i);
            }
        }
        clicked
    }

    /// Raised push button; a disabled one is drawn dim and never clicks.
    #[allow(clippy::too_many_arguments)]
    pub fn button(
        &mut self,
        key: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: &str,
        active: bool,
        ink: Color32,
        enabled: bool,
    ) -> bool {
        let cv = self.cv;
        let rect = cv.r(x, y, w, h);
        let sense = if enabled { Sense::click() } else { Sense::hover() };
        let resp = self.ui.interact(rect, Id::new(("btn", key)), sense);
        self.register(key, ControlKind::Button, rect);
        let down = resp.is_pointer_button_down_on() && enabled;
        let face = if active {
            self.th.slot
        } else if down {
            self.th.button_down
        } else if resp.hovered() && enabled {
            self.th.button_hover
        } else {
            self.th.button
        };
        let rect = if down { rect.translate(vec2(0.0, cv.s)) } else { rect };
        self.pt.rect_filled(rect.translate(vec2(0.0, 2.0 * cv.s)), cv.round(2.0), Color32::from_black_alpha(140));
        self.pt.rect_filled(rect, cv.round(2.0), face);
        if active {
            self.pt.rect_stroke(rect, cv.round(2.0), Stroke::new(1.0 * cv.s, ink), StrokeKind::Inside);
        }
        let text_ink = match (enabled, active) {
            (false, _) => self.th.legend,
            (true, true) => ink,
            (true, false) => self.th.text,
        };
        self.text(rect.center(), Align2::CENTER_CENTER, text, 9.0, text_ink);
        enabled && resp.clicked()
    }

    /// Flat header chip; the selected one is filled.
    pub fn chip(&mut self, key: &str, x: f32, y: f32, text: &str, selected: bool) -> (egui::Response, f32) {
        let cv = self.cv;
        let w = 14.0 + self.text_w(text, 9.0);
        let rect = cv.r(x, y, w, 16.0);
        let resp = self.ui.interact(rect, Id::new(key), Sense::click());
        self.register(key, ControlKind::Button, rect);
        let (fill, ink) = if selected {
            (self.th.sublabel, self.th.bar_text)
        } else if resp.hovered() {
            (self.th.button_hover, self.th.text)
        } else {
            (self.th.button, self.th.text)
        };
        self.pt.rect_filled(rect, cv.round(3.0), fill);
        self.text(rect.center(), Align2::CENTER_CENTER, text, 9.0, ink);
        (resp, w)
    }

    pub fn led(&self, c: Pos2, on: bool, color: Color32) {
        let s = self.cv.s;
        self.pt.circle_filled(c, 4.0 * s, if on { color } else { self.th.led_off });
        if on {
            self.pt.circle_filled(c, 7.0 * s, color.gamma_multiply(0.18));
        }
    }
}

/// Fills between a polyline and a horizontal baseline (screen units).
pub fn fill_to(pt: &Painter, pts: &[Pos2], base: f32, color: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = Mesh::default();
    for p in pts {
        mesh.colored_vertex(*p, color);
        mesh.colored_vertex(pos2(p.x, base), color);
    }
    for i in 0..pts.len() as u32 - 1 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    pt.add(egui::Shape::mesh(mesh));
}
