//! All DSP and shared state. Nothing in here imports nih-plug or egui, so the
//! engine is testable on its own.

pub mod auto;
pub mod eq;
pub mod fft;
pub mod filter;
pub mod shared;
pub mod spectrum;
