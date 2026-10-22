# GraphQ

A graphical parametric equalizer by Fonsie Audio, as a CLAP and VST3 plugin for Linux.
Built in Rust on [nih-plug](https://github.com/robbert-vdh/nih-plug), with the panel style
of its siblings Double Slit, Drift and the Alpha Juno clone (four colour themes, Tokyo Night
by default).

## Features

- 16 bands: **Bell**, **Low Shelf**, **High Shelf**, **Low Cut**,
  **High Cut**, **Notch**, **Band Pass** and **Tilt Shelf**.
- Frequency 10 Hz – 30 kHz, gain ±30 dB, Q 0.1 – 18.
- Cuts with 6, 12, 18, 24, 36 or 48 dB/oct (Butterworth). Q 0.71 is flat; a higher Q adds a
  resonance peak at the corner.
- Stereo placement per band: both channels, left, right, mid or side.
- Spectrum analyser before and/or after the EQ (tilted by 4.5 dB/oct so pink noise reads
  flat), output meter, output gain ±24 dB.
- Click-free editing: frequency, gain and Q glide; changing a band's type, slope or
  placement or switching it on or off cross-fades.
- Zero latency (minimum phase IIR filters).
- Automatic EQ from 10 s of the input: **resonance removal** and **pink noise balance**
  (see below).

## Automatic EQ

Two buttons in the AUTO EQ section. Each records the next 10 seconds of the plugin's input
(play the material while it records), analyses it and adds the bands it finds. Both only
ever cut. The bands they make are ordinary bands and can be edited, moved or removed like any
other; a dot in the button's colour marks them on the graph and the band chips.

1. **RESONANCES** finds narrow spikes that stand out from their surroundings for most of the
   10 seconds (at least 6 dB, Q of 3 or more, 40 Hz – 16 kHz) and cuts each with a narrow
   bell down to the surrounding level (at most 6 bands, at most -12 dB each).
2. **BALANCE (MASTER)** compares the 1/3-octave spectrum with pink noise. Where the signal
   lies above its median level it adds broad bells or shelves, largest excess first (at most
   5 bands, at most -6 dB each). Meant for the master channel.

Use them in that order: resonances first, then balance. Only one can record at a time;
clicking the recording button again cancels it. Running a feature again replaces the bands
it made before; manual bands are never touched. Each analysis takes the current effect of
all other active bands into account, so balancing after removing resonances (or after your
own edits) measures the signal as the EQ already shapes it. The bands go into bands that
are switched off; if all 16 are in use, the status line says how many could not be placed.

## Using the graph

| Action | Result |
|--------|--------|
| Double-click empty space | Add a band there (bell; a low cut near the left edge, a high cut near the right) |
| Drag a node | Frequency and gain (Shift: fine) |
| Mouse wheel on a node | Q |
| Double-click a node | Remove the band |
| Right-click a node | Pick its type, or remove it |

The strip below the graph shows the selected band: click a number to select a band,
double-click it to switch the band on or off. Knobs: drag vertically (Shift: fine), wheel,
double-click resets. The header picks the analyser mode, the graph's dB range and the theme;
these are saved with the project but are not automatable.

The graph shows the exact response of the filters that run. Bells and shelves use the RBJ
cookbook designs, so very high bands narrow slightly near Nyquist at 44.1/48 kHz; the curve
shows that. The summed curve adds all bands regardless of their stereo placement.

## Install

```sh
./install.sh              # build; symlink into ~/.clap and ~/.vst3
./install.sh --copy       # copy instead of symlink
./install.sh --system     # copy into /usr/lib/clap and /usr/lib/vst3 (sudo)
./install.sh --uninstall  # remove it again
./install.sh --help       # all options
```

Then rescan plugins in the host. The CLAP ID is `ch.fonsie.graphq`.

## Development

```sh
cargo test                                    # DSP, editor (headless) and no-allocation tests
cargo run --release --example gui_preview     # editor without a host
GRAPHQ_BANDS=1 GRAPHQ_DEMO=1 cargo run --release --example gui_preview   # with bands and a live analyser
GRAPHQ_DEMO=1 GRAPHQ_AUTO=1 cargo run --release --example gui_preview     # demo signal, resonance run started (2: balance)
```
