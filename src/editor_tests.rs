//! Headless GUI tests: the editor runs in a real egui context with simulated
//! mouse input against a mock host that applies parameter changes late, like
//! nih-plug's CLAP wrapper does, and checks gesture balance.

use super::graph::{x_of, y_of, GX0, GY0};
use super::*;
use nih_plug::wrapper::state::PluginState;
use nih_plug_egui::egui::{Event, Modifiers, PointerButton, RawInput, Vec2};

const SCREEN: Vec2 = Vec2::new(BW, BH);

#[derive(Default)]
struct MockHost {
    /// Values the "plugin" has applied (what the GUI reads back).
    store: PreviewStore,
    /// Values sent by the GUI but not applied yet.
    queue: Mutex<Vec<(ParamPtr, f32)>>,
    open_gestures: Mutex<HashMap<ParamPtr, i32>>,
    errors: Mutex<Vec<String>>,
}

impl MockHost {
    fn flush(&self) {
        let mut store = self.store.lock().unwrap();
        for (ptr, v) in self.queue.lock().unwrap().drain(..) {
            store.insert(ptr, v);
        }
    }
}

impl GuiContext for MockHost {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn request_resize(&self) -> bool {
        false
    }
    unsafe fn raw_begin_set_parameter(&self, param: ParamPtr) {
        let mut g = self.open_gestures.lock().unwrap();
        let n = g.entry(param).or_default();
        if *n != 0 {
            self.errors.lock().unwrap().push(format!("nested gesture: {}", param.name()));
        }
        *n += 1;
    }
    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        if self.open_gestures.lock().unwrap().get(&param).copied().unwrap_or(0) != 1 {
            self.errors.lock().unwrap().push(format!("set outside gesture: {}", param.name()));
        }
        self.queue.lock().unwrap().push((param, normalized));
    }
    unsafe fn raw_end_set_parameter(&self, param: ParamPtr) {
        let mut g = self.open_gestures.lock().unwrap();
        let n = g.entry(param).or_default();
        *n -= 1;
        if *n != 0 {
            self.errors.lock().unwrap().push(format!("unbalanced end: {}", param.name()));
        }
    }
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _state: PluginState) {}
}

struct Harness {
    ctx: egui::Context,
    params: GraphqParams,
    host: MockHost,
    state: EditorState,
    time: f64,
    frame: u64,
    /// Host applies queued values every `latency` frames.
    latency: u64,
    events: Vec<Event>,
    pointer: Pos2,
}

impl Harness {
    fn new(latency: u64) -> Self {
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let host = MockHost::default();
        let state = EditorState::new(Arc::new(Shared::default())).with_preview_store(host.store.clone());
        let mut h = Self {
            ctx,
            params: GraphqParams::default(),
            host,
            state,
            time: 0.0,
            frame: 0,
            latency,
            events: Vec::new(),
            pointer: pos2(0.0, 0.0),
        };
        h.run(2);
        h
    }

    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(self.time),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let setter = ParamSetter::new(&self.host);
            let (params, state) = (&self.params, &mut self.state);
            let _ = self.ctx.run(raw, |ctx| root(ctx, params, &setter, state));
            self.time += 1.0 / 60.0;
            self.frame += 1;
            if self.latency > 0 && self.frame.is_multiple_of(self.latency) {
                self.host.flush();
            }
        }
    }

    fn settle(&mut self) {
        self.run(3);
        self.host.flush();
        self.run(2);
    }

    fn move_to(&mut self, p: Pos2) {
        self.pointer = p;
        self.events.push(Event::PointerMoved(p));
    }

    fn press(&mut self, button: PointerButton, pressed: bool) {
        self.events.push(Event::PointerButton { pos: self.pointer, button, pressed, modifiers: Modifiers::default() });
    }

    fn click_with(&mut self, p: Pos2, button: PointerButton) {
        self.move_to(p);
        self.run(1);
        self.press(button, true);
        self.run(1);
        self.press(button, false);
        self.run(1);
        // Make sure the next click is not seen as a double or triple click
        // (egui counts a triple click within twice the double-click delay).
        self.time += 1.0;
    }

    fn click(&mut self, p: Pos2) {
        self.click_with(p, PointerButton::Primary);
    }

    fn double_click(&mut self, p: Pos2) {
        self.move_to(p);
        self.run(1);
        for _ in 0..2 {
            self.press(PointerButton::Primary, true);
            self.run(1);
            self.press(PointerButton::Primary, false);
            self.run(1);
        }
        self.time += 1.0;
    }

    fn drag(&mut self, from: Pos2, delta: Vec2, steps: usize) {
        self.move_to(from);
        self.run(1);
        self.press(PointerButton::Primary, true);
        self.run(1);
        for k in 1..=steps {
            self.move_to(from + delta * (k as f32 / steps as f32));
            self.run(1);
        }
        self.press(PointerButton::Primary, false);
        self.run(1);
        self.move_to(pos2(2.0, 2.0));
        self.run(1);
        self.time += 1.0;
    }

    fn wheel(&mut self, p: Pos2, y: f32) {
        self.move_to(p);
        self.run(1);
        self.events.push(Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, y), modifiers: Modifiers::default() });
        self.run(1);
    }

    fn try_control(&self, name: &str) -> Option<ControlInfo> {
        self.state.controls.iter().find(|c| c.name == name).cloned()
    }

    fn control(&self, name: &str) -> ControlInfo {
        self.try_control(name).unwrap_or_else(|| panic!("control {name} not found"))
    }

    fn center(&self, name: &str) -> Pos2 {
        self.control(name).rect.center()
    }

    /// Screen position of a frequency and gain on the graph.
    fn graph_point(&self, freq: f32, db: f32) -> Pos2 {
        let r = self.control("graph").rect;
        let s = r.width() / (graph::GX1 - GX0);
        let range = RANGES[self.params.range.load(Relaxed) as usize];
        r.min + vec2((x_of(freq) - GX0) * s, (y_of(db, range) - GY0) * s)
    }

    fn ptr(&self, id: Pid) -> ParamPtr {
        match id {
            Pid::Band(b, f) => self.params.bands[b].ptr(f),
            Pid::Output => self.params.output.as_ptr(),
        }
    }

    /// Applied value in natural units.
    fn value(&self, id: Pid) -> f32 {
        let ptr = self.ptr(id);
        let n = self
            .host
            .store
            .lock()
            .unwrap()
            .get(&ptr)
            .copied()
            .unwrap_or_else(|| unsafe { ptr.unmodulated_normalized_value() });
        unsafe { ptr.preview_plain(n) }
    }

    fn assert_clean(&self) {
        let errors = self.host.errors.lock().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let open: Vec<_> = self.host.open_gestures.lock().unwrap().iter().filter(|(_, n)| **n != 0).map(|(p, _)| unsafe { p.name().to_owned() }).collect();
        assert!(open.is_empty(), "open gestures: {open:?}");
    }
}

fn band(b: usize, f: Field) -> Pid {
    Pid::Band(b, f)
}

#[test]
fn double_click_adds_a_bell_at_the_pointer() {
    let mut h = Harness::new(2);
    let p = h.graph_point(1000.0, 6.0);
    h.double_click(p);
    h.settle();
    assert_eq!(h.value(band(0, Field::On)), 1.0);
    assert_eq!(h.value(band(0, Field::Kind)), FilterType::Bell as i32 as f32);
    assert!((h.value(band(0, Field::Freq)) / 1000.0 - 1.0).abs() < 0.02, "{}", h.value(band(0, Field::Freq)));
    assert!((h.value(band(0, Field::Gain)) - 6.0).abs() < 0.2);
    assert!(h.try_control("node_1").is_some());
    // The next one takes the next free band; near the left edge it is a low cut.
    let p = h.graph_point(15.0, 3.0);
    h.double_click(p);
    h.settle();
    assert_eq!(h.value(band(1, Field::On)), 1.0);
    assert_eq!(h.value(band(1, Field::Kind)), FilterType::LowCut as i32 as f32);
    assert!((h.value(band(1, Field::Q)) - FLAT_Q).abs() < 1e-3);
    assert_eq!(h.state.selected, 1);
    h.assert_clean();
}

#[test]
fn dragging_a_node_moves_frequency_and_gain() {
    let mut h = Harness::new(3);
    let p = h.graph_point(500.0, 0.0);
    h.double_click(p);
    h.settle();
    let from = h.center("node_1");
    let to = h.graph_point(2000.0, -6.0);
    h.drag(from, to - from, 12);
    h.settle();
    assert!((h.value(band(0, Field::Freq)) / 2000.0 - 1.0).abs() < 0.03, "{}", h.value(band(0, Field::Freq)));
    assert!((h.value(band(0, Field::Gain)) + 6.0).abs() < 0.3, "{}", h.value(band(0, Field::Gain)));
    h.assert_clean();
}

#[test]
fn wheel_on_a_node_changes_q_and_double_click_removes_it() {
    let mut h = Harness::new(1);
    let p = h.graph_point(800.0, 4.0);
    h.double_click(p);
    h.settle();
    let node = h.center("node_1");
    h.wheel(node, 50.0);
    h.settle();
    assert!(h.value(band(0, Field::Q)) > 1.05);
    h.double_click(node);
    h.settle();
    assert_eq!(h.value(band(0, Field::On)), 0.0);
    assert!(h.try_control("node_1").is_none());
    h.assert_clean();
}

#[test]
fn right_click_menu_changes_type() {
    let mut h = Harness::new(2);
    let p = h.graph_point(3000.0, 0.0);
    h.double_click(p);
    h.settle();
    let node = h.center("node_1");
    h.click_with(node, PointerButton::Secondary);
    h.run(1);
    let item = h.center("popup:4");
    h.click(item);
    h.settle();
    assert_eq!(h.value(band(0, Field::Kind)), FilterType::HighCut as i32 as f32);
    assert!(h.state.popup.is_none());
    h.assert_clean();
}

#[test]
fn band_strip_edits_the_selected_band() {
    let mut h = Harness::new(2);
    let chip = h.center("band:3");
    h.click(chip);
    assert_eq!(h.state.selected, 2);
    let on = h.center("on_3");
    h.click(on);
    let cell = h.center("type_3#3");
    h.click(cell);
    h.settle();
    assert_eq!(h.value(band(2, Field::On)), 1.0);
    assert_eq!(h.value(band(2, Field::Kind)), FilterType::LowCut as i32 as f32);
    assert!((h.value(band(2, Field::Q)) - FLAT_Q).abs() < 1e-3);
    let slope = h.center("slope_3#5");
    h.click(slope);
    let place = h.center("place_3#4");
    h.click(place);
    h.settle();
    assert_eq!(h.value(band(2, Field::Slope)), 5.0);
    assert_eq!(h.value(band(2, Field::Place)), 4.0);

    // Knobs: drag up raises, double-click resets.
    let before = h.value(band(2, Field::Freq));
    let knob = h.center("freq_3");
    h.drag(knob, vec2(0.0, -60.0), 6);
    h.settle();
    assert!(h.value(band(2, Field::Freq)) > before * 2.0);
    h.double_click(knob);
    h.settle();
    assert!((h.value(band(2, Field::Freq)) - before).abs() < 0.01);
    let out = h.center("output");
    h.drag(out, vec2(0.0, 50.0), 5);
    h.settle();
    assert!(h.value(Pid::Output) < -5.0);
    h.assert_clean();
}

#[test]
fn every_parameter_of_a_band_has_a_control() {
    let mut h = Harness::new(1);
    for b in 0..NUM_BANDS {
        let chip = h.center(&format!("band:{}", b + 1));
        h.click(chip);
        for f in [Field::On, Field::Kind, Field::Freq, Field::Gain, Field::Q, Field::Slope, Field::Place] {
            assert!(h.try_control(&band(b, f).id()).is_some(), "{}", band(b, f).id());
        }
    }
    assert!(h.try_control("output").is_some());
}

#[test]
fn header_chips_switch_theme_analyzer_and_range() {
    let mut h = Harness::new(1);
    for t in THEMES {
        let chip = h.center(&format!("theme:{}", t.name));
        h.click(chip);
        h.run(2);
    }
    assert_eq!(h.params.theme.load(Relaxed) as usize, THEMES.len() - 1);
    let chip = h.center("analyzer:OFF");
    h.click(chip);
    assert_eq!(h.params.analyzer.load(Relaxed), 0);
    let chip = h.center("range:30");
    h.click(chip);
    assert_eq!(h.params.range.load(Relaxed), 4);
    // Nodes follow the range.
    let p = h.graph_point(1000.0, 24.0);
    h.double_click(p);
    h.settle();
    assert!((h.value(band(0, Field::Gain)) - 24.0).abs() < 0.5);
}

/// 10 s of pink noise at 48 kHz, optionally with a steady tone and run
/// through one shaping band.
fn test_signal(seed: u32, tone: Option<f32>, shape: Option<crate::dsp::filter::Band>) -> Vec<f32> {
    let mut x = crate::dsp::auto::pink_noise(480_000, seed);
    if let Some(f) = tone {
        for (i, v) in x.iter_mut().enumerate() {
            *v += 0.02 * (std::f32::consts::TAU * f * i as f32 / 48_000.0).sin();
        }
    }
    if let Some(band) = shape {
        let mut bands = [crate::dsp::filter::Band::default(); NUM_BANDS];
        bands[0] = band;
        let mut eq = crate::dsp::eq::Eq::new(48_000.0);
        eq.set_params(&bands, 0.0);
        let mut r = x.clone();
        eq.process(&mut x, &mut r);
    }
    x
}

impl Harness {
    /// Plays `x` into the running capture (as the audio thread would) and
    /// lets the editor finish the run.
    fn feed(&mut self, x: &[f32]) {
        for block in x.chunks(4800) {
            self.state.shared.push_capture(block, block);
        }
        // The analysis runs on its own thread.
        let start = std::time::Instant::now();
        self.run(2);
        while self.running().is_some() && start.elapsed().as_secs() < 20 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.run(1);
        }
        self.settle();
    }

    fn running(&self) -> Option<Feature> {
        self.state.shared.job.lock().unwrap().running
    }

    fn status(&self) -> String {
        self.state.shared.job.lock().unwrap().status.clone()
    }

    fn owned(&self, f: Feature) -> Vec<usize> {
        let owners = self.params.owners();
        (0..NUM_BANDS).filter(|&b| owners[b] == f as u8 && self.value(band(b, Field::On)) > 0.5).collect()
    }
}

#[test]
fn resonances_then_balance_add_cuts_and_leave_manual_bands_alone() {
    let mut h = Harness::new(1);
    // A manual band first.
    let p = h.graph_point(5000.0, 2.0);
    h.double_click(p);
    h.settle();

    let res = h.center("auto:resonances");
    h.click(res);
    assert_eq!(h.running(), Some(Feature::Resonances));
    // The other button is locked while one runs.
    let bal = h.center("auto:balance");
    h.click(bal);
    assert_eq!(h.running(), Some(Feature::Resonances));
    h.feed(&test_signal(3, Some(1000.0), None));
    assert_eq!(h.running(), None);
    assert_eq!(h.status(), "1 RESONANCE CUT");
    let r = h.owned(Feature::Resonances);
    assert_eq!(r.len(), 1);
    assert!((h.value(band(r[0], Field::Freq)) / 1000.0 - 1.0).abs() < 0.01);
    assert!(h.value(band(r[0], Field::Gain)) < -2.0);
    assert!(h.value(band(r[0], Field::Q)) >= 4.0);
    // The manual band is untouched.
    assert!((h.value(band(0, Field::Freq)) / 5000.0 - 1.0).abs() < 0.02);
    assert_eq!(h.params.owners()[0], 0);

    // Balance a bass-heavy signal; the resonance band stays.
    let boost = crate::dsp::filter::Band { on: true, kind: FilterType::LowShelf, freq: 200.0, gain: 8.0, q: FLAT_Q, ..Default::default() };
    h.click(bal);
    assert_eq!(h.running(), Some(Feature::Balance));
    h.feed(&test_signal(5, Some(1000.0), Some(boost)));
    let b = h.owned(Feature::Balance);
    assert!(!b.is_empty(), "{}", h.status());
    assert!(h.status().starts_with("BALANCED"), "{}", h.status());
    for &i in &b {
        assert!(h.value(band(i, Field::Gain)) < 0.0);
    }
    assert_eq!(h.owned(Feature::Resonances), r);

    // A second resonance run on a clean signal removes its old cut.
    h.click(res);
    h.feed(&test_signal(7, None, None));
    assert_eq!(h.status(), "NO RESONANCES FOUND");
    assert!(h.owned(Feature::Resonances).is_empty());
    assert_eq!(h.params.owners()[r[0]], 0);
    assert_eq!(h.owned(Feature::Balance), b);
    // Auto bands stay editable: drag one.
    let node = h.center(&format!("node_{}", b[0] + 1));
    let before = h.value(band(b[0], Field::Freq));
    h.drag(node, vec2(40.0, 0.0), 6);
    h.settle();
    assert!(h.value(band(b[0], Field::Freq)) > before * 1.1);
    h.assert_clean();
}

#[test]
fn auto_run_can_be_cancelled_and_reports_silence() {
    let mut h = Harness::new(1);
    let res = h.center("auto:resonances");
    h.click(res);
    h.click(res);
    assert_eq!(h.running(), None);
    assert_eq!(h.status(), "CANCELLED");
    assert!(h.state.shared.active_capture().is_none());
    let bal = h.center("auto:balance");
    h.click(bal);
    h.feed(&vec![0.0; 480_000]);
    assert_eq!(h.status(), "NO SIGNAL");
    assert!(h.owned(Feature::Balance).is_empty());
}
