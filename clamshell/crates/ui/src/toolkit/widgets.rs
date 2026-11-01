//! Pieces that more than one view draws: a screen's header, a menu of text actions, a list's row, a row
//! of tabs, a level's track, a held slider and its card, a switch and a switch's row, a value's row, a
//! status row (for a network or a device), a tile (an icon over a caption), a progress bar, and centred
//! text.

use clamshell_state::Id;
use clamshell_state::levels::slider::Control;

use super::scroll::Scrolled;
use super::style::{self, Look, SURFACE_HI, Text, Tone};
use super::{Frame, PadKey, Track};
use crate::gfx::icons::Icon;
use crate::gfx::{Align, Canvas, Column, Keep, LINE, Rect, Rgb};

/// A row of tabs starting at `area.x`, `area.h` high. Each tab has its icon, if it has one, and its label
/// cut to `max` wide, with `inner` space on both sides. The current tab is bright with an amber line
/// under it. When they don't all fit, the row scrolls: it starts as far along as it must for the current
/// tab and the one after it to show, so a tap on the last shown brings the next in, and a dim ‹ or › at an
/// end says there are more that way. Returns where the next tab would start.
pub fn tabs<'a, I>(f: &mut Frame, area: Rect, size: f32, inner: f32, max: f32, items: I) -> f32
where
    I: IntoIterator<Item = (Id, Option<Icon>, &'a str, bool)>,
    I::IntoIter: Clone,
{
    let items = items.into_iter();
    // An icon is about as high as the label's capitals, with a space after it.
    let (icon_w, gap) = (size * 1.1, size * 0.35);
    let width = |f: &mut Frame, icon: bool, name: &str| (if icon { icon_w + gap } else { 0.0 }) + f.c.text_width(size, name).min(max) + 2.0 * inner;
    // Up to the end of the current tab and of the one after it.
    let (mut total, mut current, mut to_current, mut to_next) = (0.0, None, 0.0, 0.0);
    for (i, (_, icon, name, now)) in items.clone().enumerate() {
        total += width(f, icon.is_some(), name);
        if now {
            (current, to_current, to_next) = (Some(i), total, total);
        } else if current.is_some_and(|c| c + 1 == i) {
            to_next = total;
        }
    }
    let current = current.unwrap_or(0);
    let marker = size * 1.2;
    // Scrolled: the first tab shown is the first from which the current one and the next fit (or else the
    // current one alone), room kept for the markers.
    let (room, start) = if total <= area.w {
        (area.w, 0)
    } else {
        let room = area.w - 2.0 * marker;
        let target = if to_next - to_current <= room / 2.0 { to_next } else { to_current };
        let (mut start, mut before) = (0, 0.0);
        for (_, icon, name, _) in items.clone().take(current) {
            if target - before <= room {
                break;
            }
            before += width(f, icon.is_some(), name);
            start += 1;
        }
        (room, start)
    };
    // The ‹ takes room only when tabs are hidden before the first shown.
    let (left, room) = match (total <= area.w, start) {
        (true, _) => (area.x, room),
        (false, 0) => (area.x, room + marker),
        (false, _) => (area.x + marker, room),
    };
    let mut x = left;
    let mut hidden_after = false;
    for (id, icon, name, current) in items.clone().skip(start) {
        let lead = if icon.is_some() { icon_w + gap } else { 0.0 };
        let text_w = f.c.text_width(size, name).min(max);
        let r = Rect::new(x, area.y, lead + text_w + 2.0 * inner, area.h);
        if r.x + r.w > left + room {
            hidden_after = true;
            break;
        }
        f.button(id, r, Look::TAB, |f| {
            let tone = if current { Tone::High } else { Tone::Dim };
            if let Some(icon) = icon {
                f.c.icon(icon, Rect::new(r.x + inner, r.y + (r.h - icon_w) / 2.0, icon_w, icon_w), tone);
            }
            f.c.text_fit(Rect::new(r.x + inner + lead, r.y, text_w + 0.5, r.h), size, tone, name, Keep::Start);
            if current {
                f.c.fill(Rect::new(r.x + inner, r.y + r.h - 4.0, lead + text_w, 2.0), Tone::Accent);
            }
        });
        x += r.w;
    }
    if start > 0 {
        f.c.text_in(Rect::new(area.x, area.y, marker, area.h), size, Tone::Dim, "‹", Align::Center);
    }
    if hidden_after {
        f.c.text_in(Rect::new(left + room, area.y, marker, area.h), size, Tone::Dim, "›", Align::Center);
        return left + room + marker;
    }
    x
}

/// A held slider's sizes as fractions of `h`, the height of the bar or of a panel row: the track's
/// thickness, the thumb's radius, and the card's icon and text. `SLIDER_AWAY` is how far beside the
/// thumb the card sits, as a fraction of the width it may use.
const SLIDER_TRACK: f32 = 0.08;
const SLIDER_THUMB: f32 = 0.24;
const SLIDER_ICON: f32 = 0.62;
const SLIDER_TEXT: f32 = 0.5;
const SLIDER_AWAY: f32 = 0.12;
/// How far below the slider's middle the card's top sits (or above it, the card's bottom), as a fraction
/// of `h`.
const SLIDER_BELOW: f32 = 0.8;
/// The shade behind a held slider and its card. It is dark and see-through, so what is under them shows
/// dimmed.
pub const SHADE: Rgb = SURFACE_HI;
pub const SHADE_ALPHA: f32 = 0.85;

/// A held slider, such as volume or brightness over the bar, or a keymap parameter in its row. The
/// fields are the slider, the x its track's positions count from, the height of its middle, its size
/// (the height of the bar or the row), the room its card may use, and what the card says.
pub struct HeldSlider<'a> {
    pub slider: &'a clamshell_state::levels::slider::Slider,
    pub x0: f32,
    pub cy: f32,
    pub h: f32,
    pub within: Rect,
    pub icon: Option<Icon>,
    pub text: &'a str,
}

/// Draws a held slider (see [`HeldSlider`]). The track is drawn on a shaded band at the slider's height,
/// laid out as it was when the finger went down, with the value lit and the thumb where it is now. The
/// value is also shown on a small card where the finger doesn't cover it: below the slider, or above it
/// if there is no room below, on the side away from the finger.
pub fn held_slider(f: &mut Frame, held: HeldSlider) {
    let HeldSlider { slider, x0, cy, h, within, icon, text } = held;
    let track = Track { x0: x0 + slider.track_start(), w: slider.length };
    let thumb = h * SLIDER_THUMB;
    let at = track.x0 + track.w * slider.fraction(slider.value);
    let band = Rect::new(track.x0 - thumb * 2.0, cy - thumb * 1.8, track.w + thumb * 4.0, thumb * 3.6);
    f.c.rounded(band, band.h / 2.0, SHADE.alpha(SHADE_ALPHA));
    self::track(f.c, track, cy, h * SLIDER_TRACK, thumb, Some(slider.percent()), Tone::High);

    let side = h * SLIDER_ICON;
    let margin = side * 0.4;
    let room = within.w - margin * 2.0;
    let content_w = card_width(f, h, icon.is_some(), text, room);
    let below = cy + h * SLIDER_BELOW;
    let card_y = if below + side * 1.3 <= within.bottom() { below } else { cy - h * SLIDER_BELOW - side };
    let (wx, ww) = (within.x, within.w);
    let away = ww * SLIDER_AWAY;
    let gx = if at < wx + ww / 2.0 { at + away } else { at - away - content_w };
    let gx = gx.clamp(wx + margin, (wx + ww - content_w - margin).max(wx + margin));
    card(f, gx, card_y, h, icon, text, room);
}

/// A row whose value a finger slides, as an editor's parameters are: `label` and `value` in `tone`, and
/// `control`'s slider along `track` (the screen's width, as the bar's sliders have). The row has nothing
/// to tap, so a finger on it slides at once. While its slider is up the row shows nothing, and it returns
/// the slider, for [`held_row`] to draw after the rows, so that its card covers those below it.
pub fn slider_row(f: &mut Frame, row: Rect, size: f32, slide: (Control, Track), label: &str, value: &str, tone: Tone) -> Option<HeldRow> {
    let (control, track) = slide;
    let held = f.s.held_slider().filter(|sl| sl.control == control);
    f.button(Id::Control(control), row, Look::ROW, |f| {
        if held.is_none() {
            toned_value_row(f, row, size, label, value, tone);
        }
    });
    f.drag(super::Drag::Slide { control, track, at_once: true });
    held.map(|slider| HeldRow { row, track, slider })
}

/// A [`slider_row`] whose slider is up: the row, its track and the slider.
#[derive(Debug, Clone, Copy)]
pub struct HeldRow {
    pub row: Rect,
    pub track: Track,
    pub slider: clamshell_state::levels::slider::Slider,
}

/// Draws a [`slider_row`]'s held slider over its row, with `text` on its card (the label and the value
/// the slider is at, which the caller formats), the card within `area`. Drawn after the rows.
pub fn held_row(f: &mut Frame, held: HeldRow, area: Rect, text: &str) {
    let HeldRow { row, track, slider } = held;
    let within = Rect::new(track.x0, area.y, track.w, area.h);
    held_slider(f, HeldSlider { slider: &slider, x0: track.x0, cy: row.y + row.h / 2.0, h: row.h * 0.75, within, icon: None, text });
}

/// The width of a card's content, an icon and a line of text, at size `h`, at most `room` (see [`card`]).
pub fn card_width(f: &mut Frame, h: f32, icon: bool, text: &str, room: f32) -> f32 {
    let icon_w = if icon { h * SLIDER_ICON * 1.3 } else { 0.0 };
    (icon_w + f.c.text_width(h * SLIDER_TEXT, text)).min(room.max(icon_w))
}

/// A small shaded card over whatever is under it, with an icon and a line of text. Its content starts at
/// `x`, `y` and is sized by `h`, the height of the bar or of a panel row. Its content is at most `room`
/// wide: text past that is cut, keeping its start. Used for a held slider's value and for the bar's
/// notices.
pub fn card(f: &mut Frame, x: f32, y: f32, h: f32, icon: Option<Icon>, text: &str, room: f32) {
    let side = h * SLIDER_ICON;
    let size = h * SLIDER_TEXT;
    let icon_w = if icon.is_some() { side * 1.3 } else { 0.0 };
    let text_w = card_width(f, h, icon.is_some(), text, room) - icon_w;
    let r = Rect::new(x - side * 0.4, y - side * 0.3, icon_w + text_w + side * 0.8, side * 1.6);
    f.c.rounded(r, side * 0.3, SHADE.alpha(SHADE_ALPHA));
    if let Some(icon) = icon {
        f.c.icon(icon, Rect::new(x, y, side, side), Tone::Normal);
    }
    f.c.text_fit(Rect::new(x + icon_w, y, text_w + 1.0, side), size, Tone::High, text, Keep::Start);
}

/// A level along `track` at height `cy`, `thick` thick. The line is dim the whole way and lit up to
/// `value`, where the thumb of radius `thumb` sits. Without a value only the dim line is drawn.
pub fn track(c: &mut Canvas, track: Track, cy: f32, thick: f32, thumb: f32, value: Option<u8>, lit: Tone) {
    let Track { x0, w } = track;
    c.rounded(Rect::new(x0, cy - thick / 2.0, w, thick), thick / 2.0, Tone::Dim);
    if let Some(v) = value {
        let at = x0 + w * f32::from(v) / 100.0;
        c.rounded(Rect::new(x0, cy - thick, at - x0, thick * 2.0), thick, lit);
        c.circle(at, cy, thumb, lit);
    }
}

/// A switch's size, as fractions of its button's height.
const SWITCH_H: f32 = 0.42;
const SWITCH_W: f32 = 0.85;
/// The knob's radius, as a fraction of the switch's height.
const KNOB: f32 = 0.38;

/// An on/off switch, centred in its button `r`. When on, it is green with the knob on the right.
pub fn switch(f: &mut Frame, id: Id, r: Rect, on: bool) {
    f.button(id, r, Look::ICON, |f| switch_shape(f.c, r, on));
}

/// Draws a switch centred in `r`, sized by the height of `r`.
pub fn switch_shape(c: &mut Canvas, r: Rect, on: bool) {
    let (h, w) = (r.h * SWITCH_H, r.h * SWITCH_W);
    let track = r.centered(w, h);
    c.rounded(track, h / 2.0, if on { Tone::Go.rgb() } else { SURFACE_HI });
    let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
    c.circle(knob_x, track.y + h / 2.0, h * KNOB, if on { Tone::High } else { Tone::Dim });
}

/// What a status row shows: its icon and name in `tone`, a lock before the state if `lock`, and the
/// state in its own tone.
pub struct RowFace<'a> {
    pub icon: Icon,
    pub name: &'a str,
    pub tone: Tone,
    pub lock: bool,
    pub state: &'a str,
    pub state_tone: Tone,
}

/// A status row's icon as a fraction of the row's height, and the state's column as a fraction of its
/// width.
const ROW_ICON: f32 = 0.42;
const STATE_W: f32 = 0.22;

/// A row in a list of things that have a state, such as networks or devices. It shows the icon, the
/// name cut to fit, and the state on the right. `pad` is the list's padding and `size` its text size.
pub fn status_row(f: &mut Frame, id: Id, r: Rect, pad: f32, size: f32, face: RowFace) {
    f.button(id, r, Look::ROW, |f| {
        let icon = r.h * ROW_ICON;
        f.c.icon(face.icon, Rect::new(r.x + pad * 0.5, r.y + (r.h - icon) / 2.0, icon, icon), face.tone);
        let name_x = r.x + pad * 0.5 + icon * 1.6;
        let state_x = r.x + r.w - r.w * STATE_W;
        f.c.text_fit(Rect::new(name_x, r.y, state_x - name_x - pad, r.h), size, face.tone, face.name, Keep::Start);
        if face.lock {
            let side = icon * 0.7;
            f.c.icon(Icon::Lock, Rect::new(state_x, r.y + (r.h - side) / 2.0, side, side), Tone::Dim);
        }
        // The state is smaller than the name, with its top level with the name's.
        let ty = r.y + (r.h - size * LINE) / 2.0;
        f.c.text(state_x + icon, ty, size * 0.85, face.state_tone, face.state);
    });
}

/// Where a tile's picture goes: its size and its top, as fractions of the tile's height. `cut` says
/// whether a caption too long for the tile is cut or runs over its edges. When it is cut, the value is
/// the room left at its two sides together, in logical pixels.
pub struct TileShape {
    pub icon: f32,
    pub icon_y: f32,
    pub cut: Option<f32>,
}

impl TileShape {
    /// The picture's square in tile `r`, centred across it.
    pub fn square(&self, r: Rect) -> Rect {
        let size = r.h * self.icon;
        Rect::new(r.x + (r.w - size) / 2.0, r.y + r.h * self.icon_y, size, size)
    }
}

/// What a tile shows besides its picture: the caption under it, a dot in the top right corner (for
/// example for a game's own setting, or the game in front), and small dim text in the top left corner.
pub struct TileFace<'a> {
    pub caption: &'a str,
    pub caption_tone: Tone,
    pub dot: Option<Tone>,
    pub note: &'a str,
}

/// The caption's size and top and the corner note's size, as fractions of the tile's height.
const CAPTION: f32 = 0.15;
pub const CAPTION_Y: f32 = 0.62;
const NOTE: f32 = 0.09;

/// A tile of a grid, such as settings or the dock. It has no background. `picture` is given the tile's
/// rect and draws over the caption.
pub fn tile(f: &mut Frame, id: Id, r: Rect, shape: &TileShape, face: TileFace, picture: impl FnOnce(&mut Frame, Rect)) {
    f.button(id, r, Look::TILE, |f| {
        if let Some(tone) = face.dot {
            f.c.circle(r.x + r.w - r.h * 0.1, r.y + r.h * 0.1, r.h * 0.03, tone);
        }
        picture(f, r);
        let size = r.h * CAPTION;
        let line = Rect::new(r.x, r.y + r.h * CAPTION_Y, r.w, r.h * CAPTION * 1.6);
        match shape.cut {
            Some(margin) => {
                let w = f.c.text_width(size, face.caption).min(r.w - margin);
                let line = Rect::new(r.x + (r.w - w) / 2.0, line.y, r.w - margin, line.h);
                f.c.text_fit(line, size, face.caption_tone, face.caption, Keep::Start);
            }
            None => {
                f.c.text_in(line, size, face.caption_tone, face.caption, Align::Center);
            }
        }
        if !face.note.is_empty() {
            f.c.text(r.x + r.h * 0.07, r.y + r.h * 0.05, r.h * NOTE, Tone::Dim, face.note);
        }
    });
}

/// How far into a video the viewer is: a track `r` wide, with the part watched (`fraction`) in amber.
pub fn progress(c: &mut Canvas, r: Rect, fraction: f32) {
    c.rounded(r, r.h / 2.0, SURFACE_HI);
    let done = (r.w * fraction.clamp(0.0, 1.0)).max(r.h);
    c.rounded(Rect::new(r.x, r.y, done, r.h), r.h / 2.0, Tone::Accent);
}

/// A screen's header band and a menu's text buttons, as fractions of the body's height, and a menu's
/// width, as a fraction of the area's width.
const HEADER_H: f32 = 0.1;
const MENU_BUTTON_H: f32 = 0.085;
const MENU_W: f32 = 0.45;

/// What a header has on its right: nothing, or an action (blue text).
pub enum HeaderRight<'a> {
    None,
    Action(Id, &'a str),
    /// A dim note, as what a plot below shows.
    Note(&'a str),
}

/// A screen's header at the top of the content area. It has a back arrow on the left, whose target is
/// `back`, then the title, either after the arrow or centred, and an action on the right. Returns the
/// area below it.
pub fn header(f: &mut Frame, back: Option<Id>, title: &str, centred: bool, right: HeaderRight) -> Rect {
    let pad = f.pad();
    let mut area = style::content(f.body());
    let mut bar = area.take_top(f.body().h * HEADER_H);
    let full = bar;
    let size = f.text(Text::Heading);
    let left_w = match back {
        Some(id) => {
            let r = bar.take_left(bar.h);
            f.back(id.clone());
            f.button(id, r, Look::ICON, |f| f.c.icon(Icon::ArrowLeft, r.square_center(r.h * 0.45), Tone::Normal));
            r.w
        }
        None => 0.0,
    };
    let right_w = match right {
        HeaderRight::Action(id, label) => {
            let w = (f.c.text_width(size * 0.85, label) + pad * 2.0).min(bar.w * 0.4);
            let r = bar.take_right(w);
            f.text_button(id, r.centered(r.w, r.h * 0.8), size * 0.85, Tone::Fn, label, Align::Center);
            w
        }
        HeaderRight::Note(note) => {
            let small = f.text(Text::Small);
            let w = (f.c.text_width(small, note) + pad).min(bar.w * 0.6);
            let r = bar.take_right(w);
            f.c.text_in(r, small, Tone::Dim, note, Align::End);
            w
        }
        HeaderRight::None => 0.0,
    };
    if centred {
        let side = left_w.max(right_w);
        centred_text(f, Rect::new(full.x + side, full.y, (full.w - 2.0 * side).max(1.0), full.h), size, Tone::High, title);
    } else {
        f.c.text_fit(Rect::new(bar.x + pad * 0.4, bar.y, bar.w - pad, bar.h), size, Tone::High, title, Keep::Start);
    }
    area.y += pad * 0.4;
    area.h -= pad * 0.4;
    area
}

/// A menu of text actions, each `(target, label, tone)`, going down `area` from its top. When they don't
/// fit, as in a program's long tray menu, they continue in further columns. Items past the last column
/// that fits are left out.
pub fn menu<L: std::ops::Deref<Target = str>>(f: &mut Frame, area: Rect, items: impl IntoIterator<Item = (Id, L, Tone)>) {
    let pad = f.pad();
    let h = f.body().h * MENU_BUTTON_H;
    let size = f.text(Text::Body);
    let w = menu_w(area, size);
    let per_column = (((area.h + pad * 0.5) / (h + pad * 0.5)) as usize).max(1);
    for (i, (id, label, tone)) in items.into_iter().enumerate() {
        let (column, row) = (i / per_column, i % per_column);
        let r = Rect::new(area.x + column as f32 * (w + pad), area.y + row as f32 * (h + pad * 0.5), w, h);
        if r.right() > area.right() {
            break;
        }
        f.text_button(id, r, size, tone, &label, Align::Start);
    }
}

/// A menu's width in `area`, for text at `size`.
pub fn menu_w(area: Rect, size: f32) -> f32 {
    (area.w * MENU_W).max(size * 6.0).min(area.w)
}

/// A list's row. It shows its label in `tone`, after a dot if it is the one in force, and a dim note in
/// a column on the right. At the end there is an Edit action with a target of its own, or empty room
/// for one so the notes line up with rows that have it.
pub struct ListRow<'a> {
    pub label: &'a str,
    pub tone: Tone,
    pub mark: bool,
    pub note: &'a str,
    pub edit: EditCell,
}

/// A list row's end: nothing, an empty Edit column, or Edit.
pub enum EditCell {
    None,
    Empty,
    Edit(Id),
}

/// A list row's note column and Edit action, as fractions of the row's width.
const NOTE_W: f32 = 0.36;
const EDIT_W: f32 = 0.14;

/// A list's row (see [`ListRow`]) over `r`, with `id` as its main target and its text at `size`.
pub fn list_row(f: &mut Frame, id: Id, r: Rect, pad: f32, size: f32, row: ListRow) {
    described_list_row(f, id, r, pad, size, row, "");
}

/// The same row with an optional description below its label and note.
pub fn described_list_row(f: &mut Frame, id: Id, mut r: Rect, pad: f32, size: f32, row: ListRow, description: &str) {
    let edit = match row.edit {
        EditCell::None => None,
        EditCell::Empty => {
            r.take_right(r.w * EDIT_W);
            None
        }
        EditCell::Edit(id) => Some((id, r.take_right(r.w * EDIT_W))),
    };
    f.button(id, r, Look::ROW, |f| {
        let dot = r.h * 0.06;
        if row.mark {
            f.c.circle(r.x + pad * 0.5 + dot, r.y + r.h / 2.0, dot, Tone::Normal);
        }
        let x = r.x + pad * 0.5 + dot * 4.0;
        let label = if description.is_empty() { r } else { Rect::new(r.x, r.y, r.w, r.h * 0.55) };
        let note_w = if row.note.is_empty() { 0.0 } else { r.w * NOTE_W };
        f.c.text_fit(Rect::new(x, label.y, r.x + r.w - note_w - x - pad * 0.5, label.h), size, row.tone, row.label, Keep::Start);
        if !row.note.is_empty() {
            f.c.text_fit(Rect::new(r.x + r.w - note_w, label.y, note_w - pad * 0.5, label.h), size * 0.8, Tone::Dim, row.note, Keep::Start);
        }
    });
    if !description.is_empty() {
        let x = r.x + pad * 0.5 + r.h * 0.24;
        f.c.text_fit(Rect::new(x, r.y + r.h * 0.55, r.right() - x - pad * 0.5, r.h * 0.4), size * 0.8, Tone::Dim, description, Keep::Start);
    }
    if let Some((id, e)) = edit {
        let tone = if f.pressed(&id) { Tone::High } else { Tone::Fn };
        f.button(id, e, Look::ROW, |f| {
            f.c.text_in(e, size * 0.85, tone, "Edit", Align::Center);
        });
    }
}

/// A switch's width in a row, for the row's text size.
pub fn switch_w(size: f32) -> f32 {
    size * 2.3
}

/// A switch's row: a dim `label` on the left and the switch on the right. The whole row is its target.
pub fn switch_row(f: &mut Frame, id: Id, row: Rect, size: f32, label: &str, on: bool) {
    f.button(id, row, Look::ROW, |f| {
        let inner = row.inset(size * 0.4);
        let sw = Rect::new(inner.right() - switch_w(size), row.y, switch_w(size), row.h);
        f.c.text_fit(Rect::new(inner.x, row.y, sw.x - inner.x, row.h), size, Tone::Dim, label, Keep::Start);
        switch_shape(f.c, sw, on);
    });
}

/// A value's row: a dim `label` on the left and `value` on the right, brighter when `changed` from its
/// default. The caller provides its target, such as a held slider's or a choice's.
pub fn value_row(f: &mut Frame, row: Rect, size: f32, label: &str, value: &str, changed: bool) {
    toned_value_row(f, row, size, label, value, Tone::high_if(changed));
}

/// [`value_row`] with its value in `tone`: dim, as its label, for a value that does nothing now.
pub fn toned_value_row(f: &mut Frame, row: Rect, size: f32, label: &str, value: &str, tone: Tone) {
    let inner = row.inset(size * 0.4);
    let vw = f.c.text_width(size, value).min(inner.w * 0.5);
    f.c.text_fit(Rect::new(inner.x, row.y, inner.w - vw - size * 0.3, row.h), size, Tone::Dim, label, Keep::Start);
    f.c.text_fit(Rect::new(inner.right() - vw, row.y, vw + 1.0, row.h), size, tone, value, Keep::Start);
}

/// An action and the pad button that does it, if one does, as a target filling `r`. It shows the
/// button's name in a ring, or a pill for a longer name, then the label in `tone`. Its button presses it
/// (see [`Frame::key`]), unless faded.
pub fn key_action(f: &mut Frame, id: Id, r: Rect, key: Option<PadKey>, label: &str, tone: Tone, faded: bool) {
    if let Some(key) = key.filter(|_| !faded) {
        f.key(key, id.clone());
    }
    f.button(id, r, Look::ICON, |f| {
        key_label(f, r, key, label, tone, faded);
    });
}

/// How wide [`key_action`] draws `key`'s badge and `label` in a rect `h` high.
pub fn key_action_width(f: &mut Frame, h: f32, key: Option<PadKey>, label: &str) -> f32 {
    let d = h * 0.78;
    let ring_w = d.max(f.c.text_width(d * 0.55, badge(key)) + d * 0.6);
    // (With room to spare, so the label isn't cut at its last letter.)
    ring_w + f.pad() + f.c.text_width(h * 0.62, label)
}

/// A button's badge and what it does, drawn as [`key_action`] draws them but as a hint with nothing to
/// tap. Returns its width.
pub fn key_hint(f: &mut Frame, r: Rect, key: PadKey, label: &str) -> f32 {
    key_label(f, r, Some(key), label, Tone::Dim, false)
}

/// The name on `key`'s badge, empty without a key.
fn badge(key: Option<PadKey>) -> &'static str {
    key.map_or("", PadKey::name)
}

/// How strongly a faded badge's ring and letter show.
const FADED: f32 = 0.35;

/// Draws the badge, a ring with the button's name, and the label after it, and returns their width.
/// Without a name there is no ring, but the label stays where the other rows' labels are. The badge is
/// faded while the button doesn't do the action.
fn key_label(f: &mut Frame, r: Rect, key: Option<PadKey>, label: &str, tone: Tone, faded: bool) -> f32 {
    let key = badge(key);
    let pad = f.pad();
    let d = r.h * 0.78;
    let size = d * 0.55;
    let ring_w = d.max(f.c.text_width(size, key) + d * 0.6);
    let ring = Rect::new(r.x, r.y + (r.h - d) / 2.0, ring_w, d);
    if !key.is_empty() {
        let (ring_color, letter) =
            if faded { (Tone::Dim.rgb().alpha(FADED), Tone::Normal.rgb().alpha(FADED)) } else { (Tone::Dim.color(), Tone::Normal.color()) };
        f.c.outline(ring, d / 2.0, 1.5, ring_color);
        f.c.text_ink_centred(ring, size, letter, key);
    }
    let x = ring.x + ring_w + pad * 0.5;
    let drawn = f.c.text_fit(Rect::new(x, r.y, r.x + r.w - x, r.h), r.h * 0.62, tone, label, Keep::Start);
    x + drawn - r.x
}

/// Text centred in `r`, cut to fit.
pub fn centred_text(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
    let w = f.c.text_width(size, text).min(r.w);
    f.c.text_fit(Rect::new(r.x + (r.w - w) / 2.0, r.y, w + 1.0, r.h), size, tone, text, Keep::Start);
}

/// Text centred in `r`. If it doesn't fit on one line, it is split at the space nearest the middle onto
/// two.
pub fn centred_lines(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
    let line = size * LINE;
    if f.c.text_width(size, text) <= r.w {
        centred_text(f, Rect::new(r.x, r.y + (r.h - line) / 2.0, r.w, line), size, tone, text);
        return;
    }
    let middle = text.len() / 2;
    let split = text.match_indices(' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(middle));
    let (a, b) = match split {
        Some(i) => (text.get(..i).unwrap_or(text), text.get(i + 1..).unwrap_or("")),
        None => (text, ""),
    };
    let top = r.y + (r.h - 2.0 * line) / 2.0;
    centred_text(f, Rect::new(r.x, top, r.w, line), size, tone, a);
    centred_text(f, Rect::new(r.x, top + line, r.w, line), size, tone, b);
}

/// A hint's text size, as a fraction of the body text's.
const HINT: f32 = 0.8;

/// The height of a hint line, the small dim text at the bottom of a screen.
pub fn hint_h(f: &Frame) -> f32 {
    f.text(Text::Body) * HINT * LINE * 1.25
}

/// A small dim hint along the bottom of `area`. Returns the room above it.
pub fn hint(f: &mut Frame, mut area: Rect, text: &str) -> Rect {
    let h = hint_h(f);
    let line = area.take_bottom(h);
    let size = f.text(Text::Body) * HINT;
    f.c.text_fit(line.top_line(size), size, Tone::Dim, text, Keep::Start);
    area
}

/// A screen's scrolled list, such as networks, devices, keymaps or layers. The rows go in `area` above a
/// hint line and reach a little into the side margins for their pressed look. The scroll bar is just
/// right of them.
pub struct ListArea {
    pub pad: f32,
    /// The rows' text size.
    pub size: f32,
    pub area: Rect,
    pub row_h: f32,
}

impl ListArea {
    pub fn new(f: &Frame, area: Rect, row_h: f32) -> ListArea {
        ListArea { pad: f.pad(), size: f.text(Text::Body), area, row_h }
    }

    /// Begins the list of `len` rows above the hint line (see [`Scrolled`]).
    pub fn scrolled(&self, f: &mut Frame, list: clamshell_state::ListId, len: usize) -> Scrolled {
        let view = Rect::new(self.area.x, self.area.y, self.area.w, self.area.h - hint_h(f) - self.pad * 0.3).widened(self.pad * 0.5);
        Scrolled::begin(f, list, view, self.row_h, len)
    }

    /// Finishes the list, with its scroll bar just right of the rows.
    pub fn end(&self, f: &mut Frame, rows: Scrolled) {
        rows.end(f, self.area.right() + self.pad * 0.6);
    }

    /// The hint at the bottom.
    pub fn hint(&self, f: &mut Frame, text: &str) {
        hint(f, self.area, text);
    }

    /// A dim line where the rows would be, for when nothing has been found yet.
    pub fn empty(&self, f: &mut Frame, text: &str) {
        f.c.text(self.area.x, self.area.y + self.pad, self.size, Tone::Dim, text);
    }
}

/// Lines of text, each `(size, tone, text)`, centred across `col`'s width one under another. Lines that
/// don't fit are left out.
pub fn text_lines(f: &mut Frame, col: &mut Column, lines: &[(f32, Tone, &str)]) {
    for &(size, tone, text) in lines {
        let Some(r) = col.next(size * LINE) else {
            return;
        };
        centred_text(f, r, size, tone, text);
    }
}
