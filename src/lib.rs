//! GraphQ - a graphical parametric equalizer (CLAP and VST3).
//!
//! Two layers with a hard boundary: [`dsp`] holds the filters, the automatic
//! EQ, the analyser maths and the shared state and knows nothing about
//! nih-plug; `plugin`, `params` and `editor` adapt it to the host. They need
//! the `plugin` feature (on by default); without it the crate is the DSP
//! alone, for hosts that embed it (RasDaw's built-in GraphQ).
//!
//! Host parameters are read once per block and handed to [`dsp::eq::Eq`],
//! which glides towards them itself. The editor sees the audio through
//! [`dsp::shared::Shared`] (analyser rings, meters and the automatic EQ
//! capture, lock-free on the audio side).

pub mod dsp;
#[cfg(feature = "plugin")]
pub mod editor;
#[cfg(feature = "plugin")]
pub mod params;
#[cfg(feature = "plugin")]
mod plugin;

#[cfg(feature = "plugin")]
pub use plugin::Graphq;
