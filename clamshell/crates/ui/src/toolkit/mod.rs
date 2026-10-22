//! What every screen is drawn with: [`Frame`] (the drawing context: the canvas, the state, the targets
//! the screen lists, the sizes of the area drawn in) and the pieces built on it: [`style`] (palette,
//! sizes, looks), [`widgets`] (headers, menus, rows, sliders, switches, tiles), [`format`](mod@format) (text without
//! allocation) and [`scroll`] (scrolled lists). The screens themselves are in `views`.

pub(crate) mod format;
pub(crate) mod scroll;
pub(crate) mod style;
pub(crate) mod widgets;

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
    /// The area below the bar (the second screen): where a finger may point (see clamshell-core's `touch`).
    pub touchpad_area: Option<Rect>,
    /// How many rows each scrolled list drawn shows (by [`ListId::index`]): the state keeps the lists'
    /// offsets and the pad's focus within them.
    pub lists: [Option<f32>; ListId::COUNT],
}

/// The local time, as the clocks show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LocalTime {
    pub year: i16,
    /// 1-12.
    pub month: i8,
    pub day: i8,
    /// 0 Monday - 6 Sunday.
    pub weekday: i8,
    pub hour: i8,
    pub minute: i8,
}

impl LocalTime {
    pub const MONTHS: [&'static str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    pub const WEEKDAYS: [&'static str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

    pub fn month_name(self) -> &'static str {
        Self::MONTHS.get(usize::try_from(self.month - 1).unwrap_or(0)).copied().unwrap_or("")
    }

    pub fn weekday_name(self) -> &'static str {
        Self::WEEKDAYS.get(usize::try_from(self.weekday).unwrap_or(0)).copied().unwrap_or("")
    }

    /// The hour on a 12-hour dial (12, 1-11) and whether it is after noon.
    pub fn twelve(self) -> (i8, bool) {
        let h = self.hour % 12;
        (if h == 0 { 12 } else { h }, self.hour >= 12)
    }

    /// "14:05", or on a 12-hour dial "2:05 PM".
    pub fn time(self, twelve_hour: bool) -> Fmt<8> {
        if twelve_hour {
            let (h, pm) = self.twelve();
            Fmt::of(format_args!("{h}:{:02} {}", self.minute, if pm { "PM" } else { "AM" }))
        } else {
            Fmt::of(format_args!("{:02}:{:02}", self.hour, self.minute))
        }
    }
}

/// Kept between frames: the local time, worked out once a minute (the time zone looked up then, not
/// every frame).
#[derive(Debug, Default)]
pub struct Scratch {
    local: Option<(u64, LocalTime)>,
    /// The clock's zone, by name, as last given.
    zone: Option<(String, jiff::tz::TimeZone)>,
}

impl Scratch {
    /// The local time at `unix_secs`: in `zone` (the time tab's reading, once there is one: the system's
    /// zone as it changes), else the system's zone as the program found it.
    pub(crate) fn local(&mut self, unix_secs: u64, zone: Option<&str>) -> LocalTime {
        let minute = unix_secs / 60;
        if self.zone.as_ref().map(|(name, _)| name.as_str()) != zone {
            let tz = zone.and_then(|z| jiff::tz::db().get(z).ok());
            self.zone = zone.map(|z| (z.to_owned(), tz.unwrap_or_else(jiff::tz::TimeZone::system)));
            self.local = None;
        }
        match self.local {
            Some((m, local)) if m == minute => local,
            _ => {
                let mut local = LocalTime::default();
                if let Some(t) = i64::try_from(unix_secs).ok().and_then(|secs| jiff::Timestamp::from_second(secs).ok()) {
                    let tz = self.zone.as_ref().map_or_else(jiff::tz::TimeZone::system, |(_, tz)| tz.clone());
                    let z = t.to_zoned(tz);
                    local = LocalTime {
                        year: z.year(),
                        month: z.month(),
                        day: z.day(),
                        weekday: z.weekday().to_monday_zero_offset(),
                        hour: z.hour(),
                        minute: z.minute(),
                    };
                }
                self.local = Some((minute, local));
                local
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
