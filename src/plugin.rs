//! The CLAP/VST3 plugin: host parameters in, [`Eq`] out, analyser and
//! capture taps into [`Shared`].

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use crate::dsp::eq::Eq;
use crate::dsp::shared::Shared;
use crate::editor;
use crate::params::GraphqParams;

pub struct Graphq {
    params: Arc<GraphqParams>,
    shared: Arc<Shared>,
    eq: Eq,
}

impl Default for Graphq {
    fn default() -> Self {
        Self {
            params: Arc::new(GraphqParams::default()),
            shared: Arc::new(Shared::default()),
            eq: Eq::new(48_000.0),
        }
    }
}

impl Plugin for Graphq {
    const NAME: &'static str = "GraphQ";
    const VENDOR: &'static str = "Fonsie Audio";
    const URL: &'static str = "https://example.com/graphq";
    const EMAIL: &'static str = "beat.weber@bithawk.ch";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.shared.sample_rate.store(buffer_config.sample_rate, Relaxed);
        self.eq.set_sample_rate(buffer_config.sample_rate as f64);
        true
    }

    fn reset(&mut self) {
        self.eq.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let gui = self.params.editor_state.is_open();
        self.shared.gui_open.store(gui, Relaxed);
        self.eq.set_params(&self.params.bands(), self.params.output.value());

        let channels = buffer.as_slice();
        let (left, right) = channels.split_at_mut(1);
        let (left, right) = (&mut *left[0], &mut *right[0]);
        self.shared.push_capture(left, right);
        if gui {
            self.shared.pre.push_stereo(left, right);
        }
        self.eq.process(left, right);
        if gui {
            self.shared.post.push_stereo(left, right);
            self.shared.add_peaks(left, right);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Graphq {
    const CLAP_ID: &'static str = "ch.fonsie.graphq";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A graphical parametric equalizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Equalizer, ClapFeature::Filter, ClapFeature::Stereo];
}

impl Vst3Plugin for Graphq {
    const VST3_CLASS_ID: [u8; 16] = *b"FonsieGraphQEq01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Eq, Vst3SubCategory::Filter, Vst3SubCategory::Stereo];
}

nih_export_clap!(Graphq);
nih_export_vst3!(Graphq);
