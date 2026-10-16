//! What every screen is drawn with: [`Frame`] (the drawing context: the canvas, the state, the targets
//! the screen lists, the sizes of the area drawn in) and the pieces built on it: [`style`] (palette,
//! sizes, looks), [`widgets`] (headers, menus, rows, sliders, switches, tiles), [`format`] (text without
//! allocation) and [`scroll`] (scrolled lists). The screens themselves are in `views`.

pub(crate) mod format;
pub(crate) mod scroll;
pub(crate) mod style;
pub(crate) mod widgets;

use std::fmt::Write;

use clamshell_core::{Id, ListId, State};

use crate::draw::{Canvas, Rect};
use crate::images::Images;
use format::Fmt;
use style::{Look, PRESSED, RADIUS_S, Text, Tone};

/// Something drawn that can be tapped or focused. While drawing, in layout units; once the frame is done,
/// in the surface's logical pixels (moved by the burn-in offset).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub rect: Rect,
    pub id: Id,
    /// Where a finger on it sets a value, if it does.
    pub track: Option<Track>,
    /// The scrolled list it is in, if it is in one: a vertical drag on it scrolls the list.
    pub list: Option<scroll::InList>,
}

/// The span along x that a finger on a target sets a value over: a level row's track (0 at `x0`, 100 at
/// `x0 + w`), or for the bar's volume and brightness, the whole screen width that their slider runs
/// along. In the same units as the target's rect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Track {
    pub x0: f32,
    pub w: f32,
}

impl Track {
    /// The value (0-100) a finger at `x` sets.
    pub fn value_at(&self, x: f32) -> u8 {
        let v = (x - self.x0) / self.w.max(1.0) * 100.0;
        v.round().clamp(0.0, 100.0) as u8
    }

    /// `x` relative to where the track starts.
    pub fn along(&self, x: f32) -> f32 {
        x - self.x0
    }
}

/// What a frame tells the rest of the program besides its picture and targets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameOut {
    /// How many rows shelf's list shows (the main screen): the state keeps the selection within them.
    pub list_rows: Option<usize>,
    /// The area below the bar (the second screen), which is the touchpad in the touchpad tab.
    pub touchpad_area: Option<Rect>,
    /// How many rows each scrolled list drawn shows (by [`ListId::index`]): the state keeps the lists'
    /// offsets and the pad's focus within them.
    pub lists: [Option<f32>; ListId::COUNT],
}

/// Kept between frames: the clock's text, formatted once a minute (the time zone looked up then, not
/// every frame).
#[derive(Debug, Default)]
pub struct Scratch {
    clock: Option<(u64, Fmt<8>)>,
}

impl Scratch {
    /// The local time, "14:05".
    pub(crate) fn clock(&mut self, unix_secs: u64) -> Fmt<8> {
        let minute = unix_secs / 60;
        match self.clock {
            Some((m, text)) if m == minute => text,
            _ => {
                let mut text = Fmt::new();
                if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                    let local = t.to_zoned(jiff::tz::TimeZone::system());
                    let _ = write!(text, "{}", local.strftime("%H:%M"));
                }
                self.clock = Some((minute, text));
                text
            }
        }
    }
}

/// One screen's drawing pass: the canvas, the state it shows, images and kept text, the targets it
/// registers and what it reports.
pub struct Frame<'f, 'c> {
    pub c: &'f mut Canvas<'c>,
    pub s: &'f State,
    pub images: &'f mut Images,
    pub(crate) scratch: &'f mut Scratch,
    pub(crate) targets: &'f mut Vec<Target>,
    pub(crate) out: FrameOut,
    /// The area the screen's content goes in (the second screen's: below the bar), which its sizes are
    /// fractions of (see [`Frame::pad`], [`Frame::text`]).
    pub(crate) body: Rect,
}

impl<'f, 'c> Frame<'f, 'c> {
    pub fn new(c: &'f mut Canvas<'c>, s: &'f State, images: &'f mut Images, scratch: &'f mut Scratch, targets: &'f mut Vec<Target>) -> Self {
        targets.clear();
        Frame { c, s, images, scratch, targets, out: FrameOut::default(), body: Rect::new(0.0, 0.0, 0.0, 0.0) }
    }

    /// The area the screen's content goes in.
    pub(crate) fn body(&self) -> Rect {
        self.body
    }

    /// The padding, and the unit other spacing is counted in (see [`style::pad`]).
    pub(crate) fn pad(&self) -> f32 {
        style::pad(self.body.h)
    }

    /// A text size (see [`Text`]).
    pub(crate) fn text(&self, t: Text) -> f32 {
        t.size(self.body.h)
    }

    /// A tappable, focusable area: its background (lit while pressed), `content`, then the focus ring if
    /// focused; `rect` becomes its target.
    pub(crate) fn button(&mut self, id: Id, rect: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        self.button_in(id, rect, rect, look, content);
    }

    /// [`Frame::button`] for something drawn smaller than its target: the background and the focus ring
    /// follow `shown`, the target is `rect`.
    pub(crate) fn button_in(&mut self, id: Id, rect: Rect, shown: Rect, look: Look, content: impl FnOnce(&mut Self)) {
        let pressed = self.pressed(&id);
        let background = if pressed && look.press_fill { Some(PRESSED) } else { look.fill };
        if let Some(color) = background {
            let r = shown.inset(look.inset);
            if look.radius > 0.0 {
                self.c.rounded(r, look.radius, color);
            } else {
                self.c.fill(r, color);
            }
        }
        content(self);
        if self.s.pointer().focused(&id) {
            self.c.outline(shown.inset(look.ring), look.radius.max(RADIUS_S), 2.0, Tone::Accent);
        }
        self.targets.push(Target { rect, id, track: None, list: None });
    }

    /// Whether `id` is under a finger (or the pad's A) now: not a control whose slider is up (the
    /// finger is on the slider then, not the button).
    pub(crate) fn pressed(&self, id: &Id) -> bool {
        self.s.pointer().pressed() == Some(id) && !self.sliding(id)
    }

    /// Whether `id` is a control whose held slider is up (the bar's, a keymap parameter's).
    pub(crate) fn sliding(&self, id: &Id) -> bool {
        let Id::Control(control) = id else {
            return false;
        };
        let up = |s: Option<clamshell_core::slider::Slider>| s.is_some_and(|s| s.control == *control);
        up(self.s.levels().slider().copied()) || up(self.s.keymap_edit().slider)
    }

    /// A text action (see [`Look::TEXT`]): `label` in `r` at `size` (centred, or from the left with
    /// [`crate::draw::Align::Start`]), in `tone` (blue for an action, see [`Tone::Fn`]), white while
    /// pressed. The whole of `r` is its target.
    pub(crate) fn text_button(&mut self, id: Id, r: Rect, size: f32, tone: Tone, label: &str, align: crate::draw::Align) {
        let tone = if self.pressed(&id) { Tone::High } else { tone };
        self.button(id, r, Look::TEXT, |f| {
            let w = f.c.text_width(size, label).min(r.w);
            let x = if align == crate::draw::Align::Start { r.x } else { r.x + (r.w - w) / 2.0 };
            f.c.text_fit(Rect::new(x, r.y, w + 1.0, r.h), size, tone, label, crate::draw::Keep::Start);
        });
    }

    /// Gives the last target a track (whatever drew the track knows where it is).
    pub(crate) fn track(&mut self, track: Track) {
        if let Some(t) = self.targets.last_mut() {
            t.track = Some(track);
        }
    }

    pub fn finish(self) -> FrameOut {
        self.out
    }
}
