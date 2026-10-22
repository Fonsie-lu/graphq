//! Editor colour schemes, the same four as the sibling plugins (Double Slit,
//! Drift, the Juno clone). Never hard-code a colour elsewhere: add a field here
//! and fill it in for every theme.

use nih_plug_egui::egui::Color32;

/// Panel sections in the order of [`Theme::bars`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Graph = 0,
    Band,
    Output,
    Auto,
}

pub struct Theme {
    pub name: &'static str,
    /// Panel surface, the background around it and the bevel line along its top edge.
    pub panel: Color32,
    pub outer: Color32,
    pub bevel: Color32,
    /// Powder coat texture strength.
    pub grain: f32,
    pub fleck: f32,

    pub text: Color32,
    /// Grid lines and knob scales.
    pub scale: Color32,
    /// Grid and scale numbers.
    pub legend: Color32,
    /// Small print in the header.
    pub header: Color32,
    /// The selected header chip.
    pub sublabel: Color32,

    pub bars: [Color32; 4],
    pub bar_text: Color32,
    /// Knob arcs and selected cells.
    pub accent: Color32,
    /// Knob pointer while dragging, selection rings.
    pub drag_line: Color32,
    /// Band colours, repeating every eight bands (see [`Theme::band`]).
    pub bands: [Color32; 8],
    /// The summed EQ curve.
    pub curve: Color32,
    /// Analyser traces before and after the EQ.
    pub spectrum_pre: Color32,
    pub spectrum_post: Color32,

    pub cap: Color32,
    pub cap_bottom: Color32,
    pub slot: Color32,

    pub led: Color32,
    pub led_off: Color32,
    /// Meter segments above 0 dBFS.
    pub clip: Color32,

    pub button: Color32,
    pub button_hover: Color32,
    pub button_down: Color32,

    /// Graph display.
    pub display_bg: Color32,
    pub display_frame: Color32,
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Tokyo Night (night variant).
pub const TOKYO_NIGHT: Theme = Theme {
    name: "TOKYO NIGHT",
    panel: c(0x1a1b26),
    outer: c(0x16161e),
    bevel: c(0x3b4261),
    grain: 3.0,
    fleck: 7.0,
    text: c(0xc0caf5),
    scale: c(0x2f3549),
    legend: c(0x737aa2),
    header: c(0x737aa2),
    sublabel: c(0xff9e64),
    bars: [c(0x7dcfff), c(0xbb9af7), c(0x73daca), c(0xe0af68)],
    bar_text: c(0x16161e),
    accent: c(0x7dcfff),
    drag_line: c(0xc0caf5),
    bands: [
        c(0xf7768e),
        c(0xff9e64),
        c(0xe0af68),
        c(0x9ece6a),
        c(0x73daca),
        c(0x7dcfff),
        c(0x7aa2f7),
        c(0xbb9af7),
    ],
    curve: c(0xc0caf5),
    spectrum_pre: c(0x565f89),
    spectrum_post: c(0x7aa2f7),
    cap: c(0x24283b),
    cap_bottom: c(0x1f2335),
    slot: c(0x16161e),
    led: c(0x9ece6a),
    led_off: c(0x202915),
    clip: c(0xf7768e),
    button: c(0x3b4261),
    button_hover: c(0x414868),
    button_down: c(0x24283b),
    display_bg: c(0x13141c),
    display_frame: c(0x3b4261),
};

/// Catppuccin (Mocha flavour).
pub const CATPPUCCIN: Theme = Theme {
    name: "CATPPUCCIN",
    panel: c(0x1e1e2e),
    outer: c(0x11111b),
    bevel: c(0x45475a),
    grain: 3.0,
    fleck: 7.0,
    text: c(0xcdd6f4),
    scale: c(0x313244),
    legend: c(0x7f849c),
    header: c(0x7f849c),
    sublabel: c(0xfab387),
    bars: [c(0x89dceb), c(0xcba6f7), c(0x94e2d5), c(0xf9e2af)],
    bar_text: c(0x11111b),
    accent: c(0x89dceb),
    drag_line: c(0xcdd6f4),
    bands: [
        c(0xf38ba8),
        c(0xfab387),
        c(0xf9e2af),
        c(0xa6e3a1),
        c(0x94e2d5),
        c(0x89dceb),
        c(0x89b4fa),
        c(0xcba6f7),
    ],
    curve: c(0xcdd6f4),
    spectrum_pre: c(0x6c7086),
    spectrum_post: c(0x89b4fa),
    cap: c(0x313244),
    cap_bottom: c(0x262637),
    slot: c(0x11111b),
    led: c(0xa6e3a1),
    led_off: c(0x212d20),
    clip: c(0xf38ba8),
    button: c(0x45475a),
    button_hover: c(0x585b70),
    button_down: c(0x313244),
    display_bg: c(0x11111b),
    display_frame: c(0x45475a),
};

/// Dracula.
pub const DRACULA: Theme = Theme {
    name: "DRACULA",
    panel: c(0x282a36),
    outer: c(0x21222c),
    bevel: c(0x44475a),
    grain: 3.0,
    fleck: 7.0,
    text: c(0xf8f8f2),
    scale: c(0x373a4b),
    legend: c(0x7f86b3),
    header: c(0x7f86b3),
    sublabel: c(0xffb86c),
    bars: [c(0x8be9fd), c(0xbd93f9), c(0x50fa7b), c(0xf1fa8c)],
    bar_text: c(0x282a36),
    accent: c(0x8be9fd),
    drag_line: c(0xf8f8f2),
    bands: [
        c(0xff5555),
        c(0xffb86c),
        c(0xf1fa8c),
        c(0x50fa7b),
        c(0x8be9fd),
        c(0xbd93f9),
        c(0xff79c6),
        c(0xd6acff),
    ],
    curve: c(0xf8f8f2),
    spectrum_pre: c(0x6272a4),
    spectrum_post: c(0xbd93f9),
    cap: c(0x44475a),
    cap_bottom: c(0x343746),
    slot: c(0x1e1f29),
    led: c(0x50fa7b),
    led_off: c(0x1f3a2a),
    clip: c(0xff5555),
    button: c(0x44475a),
    button_hover: c(0x5a5d75),
    button_down: c(0x343746),
    display_bg: c(0x1e1f29),
    display_frame: c(0x44475a),
};

/// Gruvbox (dark, medium contrast).
pub const GRUVBOX: Theme = Theme {
    name: "GRUVBOX",
    panel: c(0x282828),
    outer: c(0x1d2021),
    bevel: c(0x504945),
    grain: 3.0,
    fleck: 7.0,
    text: c(0xebdbb2),
    scale: c(0x3c3836),
    legend: c(0xa89984),
    header: c(0x928374),
    sublabel: c(0xfe8019),
    bars: [c(0x8ec07c), c(0xd3869b), c(0xfabd2f), c(0xfe8019)],
    bar_text: c(0x1d2021),
    accent: c(0x8ec07c),
    drag_line: c(0xfbf1c7),
    bands: [
        c(0xfb4934),
        c(0xfe8019),
        c(0xfabd2f),
        c(0xb8bb26),
        c(0x8ec07c),
        c(0x83a598),
        c(0x458588),
        c(0xd3869b),
    ],
    curve: c(0xfbf1c7),
    spectrum_pre: c(0x7c6f64),
    spectrum_post: c(0x83a598),
    cap: c(0x3c3836),
    cap_bottom: c(0x32302f),
    slot: c(0x1d2021),
    led: c(0xb8bb26),
    led_off: c(0x2b2e14),
    clip: c(0xfb4934),
    button: c(0x504945),
    button_hover: c(0x665c54),
    button_down: c(0x3c3836),
    display_bg: c(0x1d2021),
    display_frame: c(0x504945),
};

impl Theme {
    pub fn band(&self, b: usize) -> Color32 {
        self.bands[b % self.bands.len()]
    }
}

pub const THEMES: [&Theme; 4] = [&TOKYO_NIGHT, &CATPPUCCIN, &DRACULA, &GRUVBOX];

/// Theme used for new plugin instances (index into [`THEMES`]).
pub const DEFAULT_THEME: u8 = 0;

pub fn get(index: u8) -> &'static Theme {
    THEMES[(index as usize).min(THEMES.len() - 1)]
}
