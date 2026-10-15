//! The look every screen shares: the palette, the tones text and icons come in, the second screen's text
//! sizes and spacing, corner radii, and how buttons show their states.
//!
//! OLED-friendly: black background, grey text and lines (no full white on static elements), colour only
//! where it means something (focus, the open tab, the game's own colour, Fn labels, what saves or costs
//! power).

use tiny_skia::Color;

use crate::draw::Rgb;

pub const BG: Rgb = Rgb(0x000000);
/// Fills of buttons and the selected row: barely above black.
pub const SURFACE: Rgb = Rgb(0x131316);
pub const SURFACE_HI: Rgb = Rgb(0x1f1f24);
/// A target under the finger.
pub const PRESSED: Rgb = Rgb(0x3a3a44);

/// What text and icons say by their colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Secondary, missing or off.
    Dim,
    Normal,
    /// Selected, active, connected.
    High,
    /// Focus, the open tab, something needing attention.
    Accent,
    /// Running, on, saving power; and what starts something.
    Go,
    /// The keyboard's Fn layer; settings that change how a game looks or plays.
    Fn,
}

impl Tone {
    pub fn rgb(self) -> Rgb {
        Rgb(match self {
            Tone::Dim => 0x5a5a64,
            Tone::Normal => 0xa4a4ae,
            Tone::High => 0xc8c8d0,
            Tone::Accent => 0xe0a040,
            Tone::Go => 0x45a866,
            Tone::Fn => 0x5f82b8,
        })
    }

    pub fn color(self) -> Color {
        self.rgb().into()
    }

    /// High when `on`, else normal: a selected or active row.
    pub fn high_if(on: bool) -> Tone {
        if on { Tone::High } else { Tone::Normal }
    }
}

impl From<Tone> for Color {
    fn from(t: Tone) -> Color {
        t.color()
    }
}

/// The second screen's text sizes, of the tab's area height (the area below the bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    /// The settings tab's scope tabs.
    Heading,
    /// Rows, status lines, messages.
    Body,
    /// Notes, hints, group headings, the editor's title.
    Small,
}

impl Text {
    pub fn size(self, area_h: f32) -> f32 {
        area_h
            * match self {
                Text::Heading => 0.045,
                Text::Body => 0.04,
                Text::Small => 0.032,
            }
    }
}

/// The second screen's padding, of the tab's area height: around a tab's content, and the unit its other
/// spacing is counted in.
const PAD: f32 = 0.03;

pub fn pad(area_h: f32) -> f32 {
    area_h * PAD
}

/// Where a tab's content goes in its area (`body`): padded at the sides and the bottom; the top is the
/// tab's own (a header row brings its own room).
pub fn content(body: crate::draw::Rect) -> crate::draw::Rect {
    let pad = pad(body.h);
    crate::draw::Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad)
}

/// Corner radii in logical pixels: tabs and the least a focus ring has; rows and buttons; tiles and box
/// art.
pub const RADIUS_S: f32 = 6.0;
pub const RADIUS: f32 = 8.0;
pub const RADIUS_L: f32 = 10.0;

/// How a button shows its states; the caller draws its content.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// Corner radius of the background and the focus ring (0: square, as keyboard cells).
    pub radius: f32,
    /// Background when not pressed, if any.
    pub fill: Option<Rgb>,
    /// How far inside the button's edge its background starts, in logical pixels.
    pub inset: f32,
    /// How far inside the button's edge the focus ring runs; negative: outside it (around a tile or card
    /// whose content reaches its edge).
    pub ring: f32,
}

impl Look {
    /// A list row (a level, a network, a device).
    pub const ROW: Look = Look { radius: RADIUS, fill: None, inset: 1.0, ring: 0.0 };
    /// An icon button: the bar's, a radio's switch.
    pub const ICON: Look = Look { radius: RADIUS, fill: None, inset: 2.0, ring: 0.0 };
    /// A tile of a grid (settings, the dock): no background, the ring around it.
    pub const TILE: Look = Look { radius: RADIUS_L, fill: None, inset: 0.0, ring: -2.0 };
    /// A button on a background of its own.
    pub const CARD: Look = Look { radius: RADIUS, fill: Some(SURFACE_HI), inset: 0.0, ring: -2.0 };
    /// A tab's label (shelf's collections, the settings' scopes).
    pub const TAB: Look = Look { radius: RADIUS_S, fill: None, inset: 4.0, ring: 2.0 };
    /// A keyboard key: a square cell, the ring just inside it.
    pub const KEY: Look = Look { radius: 0.0, fill: None, inset: 0.0, ring: 3.0 };
    /// Nothing shown (the dark screen).
    pub const PLAIN: Look = Look { radius: 0.0, fill: None, inset: 0.0, ring: 0.0 };
}
