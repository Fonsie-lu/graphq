//! Opens the editor in a standalone window without a plugin host. Parameter
//! changes go into a local store so every control can be operated and checked.
//!
//!   cargo run --release --example gui_preview
//!   GRAPHQ_THEME=0..3       colour scheme
//!   GRAPHQ_BANDS=1          start with a few bands switched on
//!   GRAPHQ_DEMO=1           run the EQ on pink noise with a 1.8 kHz tone and
//!                           a bass boost in a background thread at real-time
//!                           pace (no audio output), following every edit, so
//!                           the analyser, the meter and the AUTO EQ buttons work
//!   GRAPHQ_AUTO=1|2         with the demo, start the resonance (1) or balance
//!                           (2) run at once
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::{EguiWindow, GraphicsConfig};
use graphq::dsp::auto::pink_noise;
use graphq::dsp::eq::{Eq, NUM_BANDS};
use graphq::dsp::filter::{Band, FilterType, Placement, Slope};
use graphq::dsp::shared::Shared;
use graphq::editor::{self, EditorState, PreviewStore};
use graphq::params::{Field, GraphqParams};
use nih_plug::prelude::*;
use nih_plug::wrapper::state::PluginState;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct PreviewContext {
    store: PreviewStore,
}

impl GuiContext for PreviewContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn request_resize(&self) -> bool {
        false
    }
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        self.store.lock().unwrap().insert(param, normalized);
    }
    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _state: PluginState) {}
}

/// Band values for `GRAPHQ_BANDS=1`.
fn demo_bands() -> Vec<(usize, FilterType, f32, f32, f32)> {
    vec![
        (0, FilterType::LowCut, 35.0, 0.0, 0.71),
        (1, FilterType::Bell, 180.0, 3.5, 0.9),
        (2, FilterType::Bell, 420.0, -4.0, 2.2),
        (4, FilterType::Bell, 2600.0, 2.5, 1.4),
        (6, FilterType::HighShelf, 9000.0, 4.0, 0.71),
    ]
}

fn main() {
    let params = Arc::new(GraphqParams::default());
    if let Some(i) = std::env::var("GRAPHQ_THEME").ok().and_then(|v| v.parse().ok()) {
        params.theme.store(i, Relaxed);
    }
    let shared = Arc::new(Shared::default());
    shared.gui_open.store(true, Relaxed);
    let store = PreviewStore::default();
    if std::env::var("GRAPHQ_BANDS").is_ok() {
        let mut s = store.lock().unwrap();
        for (b, kind, freq, gain, q) in demo_bands() {
            let bp = &params.bands[b];
            let mut put = |f: Field, v: f32| {
                let ptr = bp.ptr(f);
                s.insert(ptr, unsafe { ptr.preview_normalized(v) });
            };
            put(Field::On, 1.0);
            put(Field::Kind, kind as i32 as f32);
            put(Field::Freq, freq);
            put(Field::Gain, gain);
            put(Field::Q, q);
        }
    }
    let auto = std::env::var("GRAPHQ_AUTO").ok().and_then(|v| v.parse::<u8>().ok());
    if let Some(feature) = auto.and_then(graphq::dsp::auto::Feature::from_tag) {
        shared.start_capture(48_000.0);
        shared.job.lock().unwrap().running = Some(feature);
    }
    if std::env::var("GRAPHQ_DEMO").is_ok() {
        let (shared, params, store) = (shared.clone(), params.clone(), store.clone());
        std::thread::spawn(move || demo(shared, params, store));
    }
    let settings = WindowOpenOptions {
        title: "GraphQ Preview".into(),
        size: Size::new(editor::WIDTH as f64, editor::HEIGHT as f64),
        scale: WindowScalePolicy::ScaleFactor(1.0),
        gl_config: Some(Default::default()),
    };
    let context = PreviewContext { store: store.clone() };
    EguiWindow::open_blocking(
        settings,
        GraphicsConfig::default(),
        EditorState::new(shared).with_preview_store(store),
        |ctx, _queue, _state| editor::setup_style(ctx),
        move |ctx, _queue, state| {
            let setter = ParamSetter::new(&context);
            editor::root(ctx, &params, &setter, state);
        },
    );
}

/// Pink noise with a steady tone (a resonance) and a bass boost (unbalanced)
/// through the EQ, with the window's current parameter values, published as
/// the plugin would.
fn demo(shared: Arc<Shared>, params: Arc<GraphqParams>, store: PreviewStore) {
    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;
    let mut eq = Eq::new(SR as f64);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let source = {
        let mut x = pink_noise(SR as usize * 20, 1);
        for (i, v) in x.iter_mut().enumerate() {
            *v = *v * 2.0 + 0.02 * (std::f32::consts::TAU * 1800.0 * i as f32 / SR).sin();
        }
        let mut boost = [Band::default(); NUM_BANDS];
        boost[0] = Band { on: true, kind: FilterType::LowShelf, freq: 180.0, gain: 7.0, q: 0.71, ..Band::default() };
        let mut shape = Eq::new(SR as f64);
        shape.set_params(&boost, 0.0);
        let mut y = x.clone();
        shape.process(&mut x, &mut y);
        x
    };
    let start = Instant::now();
    let mut rendered = 0usize;
    loop {
        let bands: [Band; NUM_BANDS] = {
            let s = store.lock().unwrap();
            std::array::from_fn(|b| {
                let bp = &params.bands[b];
                let v = |f: Field| {
                    let ptr = bp.ptr(f);
                    let n = s.get(&ptr).copied().unwrap_or_else(|| unsafe { ptr.unmodulated_normalized_value() });
                    unsafe { ptr.preview_plain(n) }
                };
                Band {
                    on: v(Field::On) > 0.5,
                    kind: FilterType::from_index(v(Field::Kind) as i32),
                    freq: v(Field::Freq),
                    gain: v(Field::Gain),
                    q: v(Field::Q),
                    slope: Slope::from_index(v(Field::Slope) as i32),
                    place: Placement::from_index(v(Field::Place) as i32),
                }
            })
        };
        let out = {
            let ptr = params.output.as_ptr();
            let n = store.lock().unwrap().get(&ptr).copied().unwrap_or(0.5);
            unsafe { ptr.preview_plain(n) }
        };
        eq.set_params(&bands, out);
        for i in 0..BLOCK {
            let x = source[(rendered + i) % source.len()];
            l[i] = x;
            r[i] = x;
        }
        shared.push_capture(&l, &r);
        shared.pre.push_stereo(&l, &r);
        eq.process(&mut l, &mut r);
        shared.post.push_stereo(&l, &r);
        shared.add_peaks(&l, &r);
        rendered += BLOCK;
        let due = Duration::from_secs_f32(rendered as f32 / SR);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
}
