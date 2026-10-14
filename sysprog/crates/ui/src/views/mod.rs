//! The screens. Each is a function of the state and its size that draws everything and lists what can
//! be tapped or focused (immediate mode; docs/plans.md, "UI code"). One module per screen; what they
//! share is here: the palette, text helpers, and [`Frame::button`], which gives every tappable thing the
//! same pressed highlight, focus ring and target.
//!
//! OLED-friendly: black background, grey text and lines (no full white on static elements), colour only
//! where it means something (focus, the active mode, the game's own colour, Fn labels, Launch).
//!
//! Sizes are fractions of the screen or of the element they sit in, named per module.

mod bar;
mod keyboard;
mod settings;
mod shelf;

use std::fmt::Write;

use sysprog_core::{Id, Mode, State};

use crate::draw::{Canvas, Rect, rgb};

/// Something drawn that can be tapped or focused, in the view's logical pixels.
#[derive(Debug, Clone)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
}

const BG: u32 = 0x000000;
/// Fills of buttons and the selected row: barely above black.
const SURFACE: u32 = 0x131316;
const SURFACE_HI: u32 = 0x1f1f24;
/// A target under the finger.
const PRESSED: u32 = 0x3a3a44;
const TEXT: u32 = 0xa4a4ae;
const TEXT_HI: u32 = 0xc8c8d0;
const DIM: u32 = 0x5a5a64;
const ACCENT: u32 = 0xe0a040;
const GO: u32 = 0x45a866;
/// Fn-layer labels: a muted blue, to tell them from the Shift layer's grey.
const FN: u32 = 0x5f82b8;

/// Text kept between frames so drawing doesn't allocate: the clock, formatted once a minute.
#[derive(Debug, Default)]
pub struct Scratch {
    minute: Option<u64>,
    clock: String,
}

impl Scratch {
    fn clock(&mut self, unix_secs: u64) -> &str {
        let minute = unix_secs / 60;
        if self.minute != Some(minute) {
            self.minute = Some(minute);
            self.clock.clear();
            if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                let local = t.to_zoned(jiff::tz::TimeZone::system());
                let _ = write!(self.clock, "{}", local.strftime("%H:%M"));
            }
        }
        &self.clock
    }
}

/// One screen's drawing pass: the canvas, the state it shows, and the targets it registers.
struct Frame<'f, 'c> {
    c: &'f mut Canvas<'c>,
    s: &'f State,
    targets: &'f mut Vec<Target>,
}

/// How a button shows its states; the caller draws its content.
#[derive(Debug, Clone, Copy)]
struct Look {
    /// Corner radius of the background and the focus ring (0: square, as keyboard cells).
    radius: f32,
    /// Background when not pressed, if any.
    fill: Option<u32>,
    /// How far inside the button its background starts (negative: outside).
    inset: f32,
    /// How far inside the button the focus ring is drawn (negative: outside).
    ring_inset: f32,
}

impl Frame<'_, '_> {
    /// A tappable, focusable area: its background (lit while pressed), `content`, then the focus ring if
    /// focused; `rect` becomes its target.
    fn button(&mut self, id: Id, rect: Rect, look: Look, content: impl FnOnce(&mut Canvas)) {
        let pressed = self.s.pressed.as_ref() == Some(&id);
        let background = if pressed { Some(PRESSED) } else { look.fill };
        if let Some(color) = background {
            let r = rect.inset(look.inset);
            if look.radius > 0.0 {
                self.c.rounded(r, look.radius, rgb(color));
            } else {
                self.c.fill(r, rgb(color));
            }
        }
        content(self.c);
        if self.s.focus.as_ref() == Some(&id) {
            self.c.outline(rect.inset(look.ring_inset), look.radius.max(6.0), 2.0, rgb(ACCENT));
        }
        self.targets.push(Target { rect, id });
    }
}

/// Text centred in `r`.
fn label(c: &mut Canvas, r: Rect, size: f32, color: u32, text: &str) {
    let w = c.text_width(size, text);
    c.text(r.x + (r.w - w) / 2.0, r.y + (r.h - size * 1.2) / 2.0, size, rgb(color), text);
}

/// The main screen: shelf.
pub fn top(c: &mut Canvas, s: &State, w: f32, h: f32, targets: &mut Vec<Target>) {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    targets.clear();
    shelf::draw(&mut Frame { c, s, targets }, w, h);
}

/// The second screen: the bar, then the current mode below it.
pub fn bottom(c: &mut Canvas, s: &State, w: f32, h: f32, scratch: &mut Scratch, targets: &mut Vec<Target>) {
    c.fill(Rect::new(0.0, 0.0, w, h), rgb(BG));
    targets.clear();
    let f = &mut Frame { c, s, targets };
    let bar_h = bar::draw(f, w, h, scratch);
    let body = Rect::new(0.0, bar_h, w, h - bar_h);
    match s.mode {
        Mode::Follow => settings::draw(f, body),
        Mode::Keyboard => keyboard::draw(f, body),
        Mode::Launcher => label(f.c, body, body.h * 0.05, DIM, "Launcher: not built yet"),
        Mode::Touchpad => label(f.c, body, body.h * 0.05, DIM, "Touchpad: not built yet"),
        Mode::Sysinfo => label(f.c, body, body.h * 0.05, DIM, "Sysinfo: not built yet"),
    }
}
