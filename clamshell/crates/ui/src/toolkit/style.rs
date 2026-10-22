//! The look every screen shares: the palette, the tones text and icons come in, the second screen's text
//! sizes and spacing, corner radii, and how buttons show their states.
//!
//! The look is OLED-friendly. The background is black, text and lines are grey, and nothing static is
//! full white. Colour is used only where it means something: focus, the open tab, the game's own colour,
//! Fn labels, and what saves or costs power.

use tiny_skia::Color;

use crate::gfx::Rgb;

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
    /// What takes something away (the keyboard's Backspace).
    Stop,
    /// The keyboard's Fn layer, settings that change how a game looks or plays, and what a keymap remaps.
    /// Also actions, meaning text that does something when tapped. An action has no fill or border, and
    /// is white while pressed or when it is the current option of a choice.
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
            // The terminal's red, from foot.ini.
            Tone::Stop => 0xc0605a,
            Tone::Fn => 0x5f82b8,
        })
    }

    pub fn color(self) -> Color {
        self.rgb().into()
    }

    /// High when `on`, else normal; for a row that may be selected or active.
    pub fn high_if(on: bool) -> Tone {
        if on { Tone::High } else { Tone::Normal }
    }
}

impl From<Tone> for Color {
    fn from(t: Tone) -> Color {
        t.color()
    }
}

/// The second screen's text sizes, as fractions of the height of the tab's area below the bar.
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

/// The second screen's padding, as a fraction of the tab's area height. It goes around a tab's content
/// and is the unit the tab's other spacing is counted in.
const PAD: f32 = 0.03;

pub fn pad(area_h: f32) -> f32 {
    area_h * PAD
}

/// Where a tab's content goes in its area (`body`). It is padded at the sides and the bottom. The top is
/// left to the tab, since a header row brings its own room.
pub fn content(body: crate::gfx::Rect) -> crate::gfx::Rect {
    let pad = pad(body.h);
    crate::gfx::Rect::new(body.x + pad, body.y, body.w - 2.0 * pad, body.h - pad)
}

/// The gap between tiles in a grid, such as the settings' or the dock's, as a fraction of the area's
/// height.
pub const TILE_GAP: f32 = 0.014;

/// Corner radii in logical pixels. `RADIUS_S` is for tabs and is the smallest a focus ring has, `RADIUS`
/// is for rows and buttons, and `RADIUS_L` is for tiles and box art.
pub const RADIUS_S: f32 = 6.0;
pub const RADIUS: f32 = 8.0;
pub const RADIUS_L: f32 = 10.0;

/// How a button shows its states; the caller draws its content.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    /// Corner radius of the background and the focus ring. 0 is square, as for keyboard cells.
    pub radius: f32,
    /// Background when not pressed, if any.
    pub fill: Option<Rgb>,
    /// How far inside the button's edge its background starts, in logical pixels.
    pub inset: f32,
    /// How far inside the button's edge the focus ring runs. A negative value puts it outside, for a tile
    /// or card whose content reaches its edge.
    pub ring: f32,
    /// Whether a press shows as a fill ([`PRESSED`]). If not, the content shows it, as a text action's
    /// label brightens.
    pub press_fill: bool,
}

impl Look {
    /// A list row (a level, a network, a device).
    pub const ROW: Look = Look { radius: RADIUS, fill: None, inset: 1.0, ring: 0.0, press_fill: true };
    /// An icon button: the bar's, a radio's switch.
    pub const ICON: Look = Look { radius: RADIUS, fill: None, inset: 2.0, ring: 0.0, press_fill: true };
    /// A tile of a grid, such as settings or the dock. It has no background, and the ring goes around it.
    pub const TILE: Look = Look { radius: RADIUS_L, fill: None, inset: 0.0, ring: -2.0, press_fill: true };
    /// A text action on the black. It has no background or border, and the label is blue ([`Tone::Fn`]),
    /// white while pressed or when it is the chosen option of a choice. The target is as large as the
    /// room it is given.
    pub const TEXT: Look = Look { radius: RADIUS, fill: None, inset: 0.0, ring: -2.0, press_fill: false };
    /// A button on a background of its own.
    pub const CARD: Look = Look { radius: RADIUS, fill: Some(SURFACE_HI), inset: 0.0, ring: -2.0, press_fill: true };
    /// A tab's label (shelf's collections, the settings' scopes).
    pub const TAB: Look = Look { radius: RADIUS_S, fill: None, inset: 4.0, ring: 2.0, press_fill: true };
    /// A keyboard key: a square cell with the ring just inside it.
    pub const KEY: Look = Look { radius: 0.0, fill: None, inset: 0.0, ring: 3.0, press_fill: true };
    /// Nothing shown (the dark screen).
    pub const PLAIN: Look = Look { radius: 0.0, fill: None, inset: 0.0, ring: 0.0, press_fill: true };
}
