# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

GraphQ (vendor Fonsie Audio) is a graphical 16-band parametric EQ plugin with an automatic EQ (resonance removal, pink noise balance) from a 10 s capture, CLAP + VST3, built on nih-plug with a custom-painted nih_plug_egui editor (egui 0.31). It follows the sibling project `~/tools/double-slit` in structure, build, install script and GUI style (same four themes, fonts, `Cv`/`G` painting helpers, headless editor tests). It is meant to be hosted by RasDaw (`~/tools/daw`) and other CLAP/VST3 hosts. RasDaw also embeds the DSP directly as its built-in device `rasdaw.graphq` (path dependency, `default-features = false`): keep `src/dsp/` free of nih-plug and run `cargo test --no-default-features --lib` after changing its public API, then `cargo test` in `~/tools/daw`.

## Commands

```sh
cargo test                                   # DSP + headless editor + tests/no_alloc.rs
cargo test --lib dsp::filter                 # one module's tests
cargo test --lib editor::tests::band_strip   # editor tests by name filter
cargo clippy --all-targets

cargo xtask bundle graphq --release          # -> target/bundled/GraphQ.{clap,vst3}
./install.sh                                 # build, symlink into ~/.clap and ~/.vst3 (--help for options)

cargo run --release --example gui_preview    # editor without a host
GRAPHQ_BANDS=1 GRAPHQ_DEMO=1 GRAPHQ_THEME=0..3 cargo run --release --example gui_preview
GRAPHQ_DEMO=1 GRAPHQ_AUTO=1|2 cargo run --release --example gui_preview   # start a resonance / balance run at once
cargo test --lib print_examples -- --ignored --nocapture                 # what the auto analyses make of test signals
```

`cargo xtask` is an alias in `.cargo/config.toml`. `[profile.test]` uses opt-level 2. ALSA/JACK dev headers are not installed, so there is no nih-plug standalone; use `gui_preview` (`GRAPHQ_DEMO=1` runs the EQ on pink noise with a 1.8 kHz tone and a bass boost in a thread, no audio out, so the analyser, the meter and the AUTO EQ buttons work). The only compiler warnings left come from `nih_export_vst3!` inside nih-plug.

Screenshot workflow: start the preview with `timeout 9 ... &`, wait ~6 s, `import -window "GraphQ Preview" out.png`; the window manager makes the window taller than the canvas, so crop (`magick out.png -crop 1122x604+0+220 +repage crop.png` for the 1122x1044 window) and Read the PNG. Do not take a screenshot in the moment a capture completes: `import` during the analysis has frozen the preview here. In RasDaw: `cargo run --example editor_smoke -- ch.fonsie.graphq 5` from `~/tools/daw`.

## Architecture

Two layers with a hard boundary. The `plugin` cargo feature (default) gates `plugin.rs` (the nih-plug `Plugin`, CLAP/VST3 exports), `params` and `editor`; `gui_preview` and `tests/no_alloc.rs` require it.

- **`src/dsp/`**: filters, the EQ processor, analyser maths and shared state. **No nih-plug or egui imports.**
  - `filter.rs`: `Band` (natural units), `FilterType`/`Slope`/`Placement` with their display names, `design()` (RBJ biquads; cuts as Butterworth cascades, the most resonant pole pair scaled by Q/0.71) and `Design::response_db()`, the exact digital response the graph draws.
  - `eq.rs`: `Eq` runs the 8 bands in series in TDF-II f64. Frequency/gain/Q glide (log domain, 20 ms) with coefficients redesigned every `CHUNK` = 16 samples. Type, slope or placement changes and on/off fade the band to dry (6 ms), swap the structure while silent, then fade back in. A band with wet 0 is skipped, so disabled bands pass audio bit-exact.
  - `shared.rs`: `Shared` (one `Arc`): pre/post analyser rings (atomics, 8192 samples), sample rate, output peaks (only while the editor is open), the auto EQ capture slot, `job` and `result`. The audio thread only stores; the mutexes are for the GUI and analysis threads.
  - `auto.rs`: automatic EQ. `Capture` (10 s mono, atomics; the audio thread appends via `Shared::push_capture`, an `AtomicPtr` into an `Arc` that `Shared::captures` keeps alive for its whole life, reused per sample rate, so the audio thread never frees). `find_resonances` (Welch average, ~3 Hz bins; median baseline ±1/3 oct; prominence ≥ 6 dB, -3 dB width Q ≥ 3, sticks out by 3 dB in ≥ 50 % of frames; bell down to the baseline) and `balance` (1/3-octave levels every 1/12 oct tilted +3 dB/oct so pink is flat; excess over the median of 40 Hz–12 kHz; greedy bells/shelves, ≤ 6 dB each, until the excess is < 1.5 dB). Both add the response of the bands they do not own (`others`) and return only cuts. `pink_noise` (Kellet) feeds the tests and the preview demo.
  - `spectrum.rs`: FFT (`fft.rs`, copied from Double Slit) of the ring, sampled at the graph's log-spaced points (max of the bins per point, interpolation where points are denser than bins), +4.5 dB/oct display tilt, rise/fall smoothing.
- **`src/plugin.rs`, `src/params.rs`, `src/editor/`**: the plugin, host parameters and GUI. `process` reads all parameters once per block (`GraphqParams::bands()`), feeds the capture (input, before the EQ), calls `Eq::set_params`, then `Eq::process`; nih-plug smoothers are not used.

### Parameters

`params.rs` uses `#[derive(Params)]` with `#[nested(array, group = "Band")] bands: [BandParams; 8]`, so IDs are `on_1`, `type_1`, `freq_1`, `gain_1`, `q_1`, `slope_1`, `place_1` … `_8`, plus `output`. These IDs are the automation and state keys: never rename one (`params::tests::ids_are_stable`). Choices are `IntParam`s whose text comes from the `NAMES` tables in `dsp/filter.rs`; never reorder those enums. Editor settings that are saved but not automatable are `#[persist]` fields: `theme`, `analyzer` (index into `editor::ANALYZER_MODES`), `range` (index into `editor::RANGES`) and `auto` (`auto_owner`: per band the `Feature` tag that made it, 0 = manual; read through `owners()`/`set_owners()`, padded if a saved list has another length).

### Editor (`src/editor/`)

Painted on a fixed 1200×640 canvas (`BW`/`BH`) through `Cv`, scaled to the window. `mod.rs`: state, header, band strip, AUTO EQ section, output section, type popup; `graph.rs`: grid, analyser, curves, nodes (drag/wheel/double-click/right-click), add-band logic, meter; `widgets.rs`: `Cv`, `G` (parameter I/O, knob, segments, button, chip), `Pid`.

- Parameters are addressed as `Pid::Band(index, Field)` or `Pid::Output` and accessed through `ParamPtr` (`G::ptr`), so all I/O is generic. Values reach the plugin asynchronously: read through `G::shown`/`plain`/`band` (pending values win until the host applies them) and write only through `send` inside `begin`/`end`, or `change`. Knobs and node drags use `drag_offset` (offset from the press origin) because egui swallows the first pixels of a drag.
- Knob travel is logarithmic for frequency and Q (`Pid::log_travel`), independent of the host's skewed range.
- Graph mapping: `x_of`/`freq_at` (log, 10 Hz – 30 kHz), `y_of`/`db_at` at the selected range. Nodes of types without gain sit on the 0 dB line and drag horizontally only.
- Never hard-code colours; add a `Theme` field (`theme.rs`) and fill it for all four themes. `Theme::band(b)` gives band colours (eight, repeating).
- Auto EQ flow, shared with RasDaw: `Shared::begin_run` / `cancel_run`; `poll_run` every frame (when the capture is full it starts the analysis thread with the bands the feature does not own; returns the result once, only for the current run id); the caller applies it with `auto::plan` (the feature's own bands first, then switched-off ones; leftover own bands off; new owner tags) and calls `finish_run(auto::status(..))`; `run_state` for drawing. The editor does this in `auto_section`/`apply_auto`. One run at a time; clicking the running button cancels. The state lives in `Shared`, so a run survives closing the editor, but its bands are applied only once the editor is open again.
- `src/editor_tests.rs` (compiled as `editor::tests` via `#[path]`) runs the real editor headless against a mock host that applies values late and checks gesture balance. Controls register their rects in `EditorState::controls` under the parameter ID (cells as `"<id>#<i>"`, nodes as `node_<n>`, chips as `band:<n>`, `theme:<NAME>`, `analyzer:<MODE>`, `range:<dB>`, popup entries as `popup:<i>`, `auto:resonances`, `auto:balance`). `Harness::feed` plays a signal into the capture and waits for the analysis thread. Clicks in the harness are spaced 1 s apart: egui counts a triple click within twice the double-click delay.

## Conventions

- No allocation or lock on the audio path; `tests/no_alloc.rs` proves it with nih-plug's aborting allocator while switching every type, slope and placement.
- No vendor name or logo on the panel (the product name in the header is fine).
