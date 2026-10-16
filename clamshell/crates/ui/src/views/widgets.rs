//! Pieces more than one view draws: a screen's header, a menu of text actions, a list's row, a row of
//! tabs, a level's track, a held slider and its card, a switch and a switch's row, a value's row, a
//! status row (a network, a device), a tile (an icon over a caption), a game's box art, and centred
//! text.

use clamshell_core::{Game, Id};

use super::style::{self, Look, SURFACE_HI, Text, Tone};
use super::{Frame, Track};
use crate::draw::{Align, Canvas, Keep, LINE, Rect, Rgb};
use crate::icons::Icon;
use crate::images::Images;

/// A row of tabs from `area.x`, `area.h` high: each its label (cut to `max` wide) with `inner` space on
/// both sides; the current one bright with an amber line under it. Tabs that would pass the area's right
/// edge are left out. Returns where a next one would start.
pub(super) fn tabs<'a>(f: &mut Frame, area: Rect, size: f32, inner: f32, max: f32, items: impl IntoIterator<Item = (Id, &'a str, bool)>) -> f32 {
    let mut x = area.x;
    for (id, name, current) in items {
        let text_w = f.c.text_width(size, name).min(max);
        let r = Rect::new(x, area.y, text_w + 2.0 * inner, area.h);
        if r.x + r.w > area.x + area.w {
            break;
        }
        f.button(id, r, Look::TAB, |f| {
            let tone = if current { Tone::High } else { Tone::Dim };
            f.c.text_fit(Rect::new(r.x + inner, r.y, text_w + 0.5, r.h), size, tone, name, Keep::Start);
            if current {
                f.c.fill(Rect::new(r.x + inner, r.y + r.h - 4.0, text_w, 2.0), Tone::Accent);
            }
        });
        x += r.w;
    }
    x
}

/// A held slider's sizes, of `h` (the bar's height, a panel row's): the track's thickness, the thumb's
/// radius, a card's icon and text; how far beside the thumb its card sits, of the width it may use.
const SLIDER_TRACK: f32 = 0.08;
const SLIDER_THUMB: f32 = 0.24;
const SLIDER_ICON: f32 = 0.62;
const SLIDER_TEXT: f32 = 0.5;
const SLIDER_AWAY: f32 = 0.12;
/// A held slider's card: its top below the slider's middle (or its bottom above it), of `h`.
const SLIDER_BELOW: f32 = 0.8;
/// Behind a held slider and its card: dark and see-through, so what is under them shows, dimmed.
pub(super) const SHADE: Rgb = SURFACE_HI;
pub(super) const SHADE_ALPHA: f32 = 0.85;

/// A held slider (volume and brightness over the bar, a keymap parameter in its row): on a shaded band
/// at height `cy`, the track laid out from `x0` with the value it had under the finger (the value lit,
/// the thumb where it is now), and the value (`icon` and `text`) on a small card of its own where the
/// finger doesn't cover it: below the slider (above it without room below, in `within`), to the side
/// away from the finger.
#[allow(clippy::too_many_arguments)]
pub(super) fn held_slider(f: &mut Frame, slider: &clamshell_core::slider::Slider, x0: f32, cy: f32, h: f32, within: Rect, icon: Option<Icon>, text: &str) {
    let track = Track { x0: x0 + slider.track_start(), w: slider.length };
    let thumb = h * SLIDER_THUMB;
    let at = track.x0 + track.w * slider.fraction(slider.value);
    let band = Rect::new(track.x0 - thumb * 2.0, cy - thumb * 1.8, track.w + thumb * 4.0, thumb * 3.6);
    f.c.rounded(band, band.h / 2.0, SHADE.alpha(SHADE_ALPHA));
    self::track(f.c, track, cy, h * SLIDER_TRACK, thumb, Some(slider.percent()), Tone::High);

    let content_w = card_width(f, h, icon.is_some(), text);
    let side = h * SLIDER_ICON;
    let margin = side * 0.4;
    let below = cy + h * SLIDER_BELOW;
    let card_y = if below + side * 1.3 <= within.bottom() { below } else { cy - h * SLIDER_BELOW - side };
    let (wx, ww) = (within.x, within.w);
    let away = ww * SLIDER_AWAY;
    let gx = if at < wx + ww / 2.0 { at + away } else { at - away - content_w };
    let gx = gx.clamp(wx + margin, (wx + ww - content_w - margin).max(wx + margin));
    card(f, gx, card_y, h, icon, text);
}

/// A card's content width (an icon and a line of text), at size `h` (see [`card`]).
pub(super) fn card_width(f: &mut Frame, h: f32, icon: bool, text: &str) -> f32 {
    let icon_w = if icon { h * SLIDER_ICON * 1.3 } else { 0.0 };
    icon_w + f.c.text_width(h * SLIDER_TEXT, text)
}

/// A small card of its own over whatever is under it (shaded): an icon and a line of text, its content
/// from `x` at `y`, sized by `h` (the bar's height, a panel row's). A held slider's value and the bar's
/// notices.
pub(super) fn card(f: &mut Frame, x: f32, y: f32, h: f32, icon: Option<Icon>, text: &str) {
    let side = h * SLIDER_ICON;
    let size = h * SLIDER_TEXT;
    let icon_w = if icon.is_some() { side * 1.3 } else { 0.0 };
    let text_w = f.c.text_width(size, text);
    let r = Rect::new(x - side * 0.4, y - side * 0.3, icon_w + text_w + side * 0.8, side * 1.6);
    f.c.rounded(r, side * 0.3, SHADE.alpha(SHADE_ALPHA));
    if let Some(icon) = icon {
        f.c.icon(icon, Rect::new(x, y, side, side), Tone::Normal);
    }
    f.c.text_in(Rect::new(x + icon_w, y, text_w + 1.0, side), size, Tone::High, text, Align::Start);
}

/// A level along `track` at height `cy`, `thick` thick: dim the whole way, lit up to `value` with the
/// thumb (`thumb` its radius) there; only the dim line without a value.
pub(super) fn track(c: &mut Canvas, track: Track, cy: f32, thick: f32, thumb: f32, value: Option<u8>, lit: Tone) {
    let Track { x0, w } = track;
    c.rounded(Rect::new(x0, cy - thick / 2.0, w, thick), thick / 2.0, Tone::Dim);
    if let Some(v) = value {
        let at = x0 + w * f32::from(v) / 100.0;
        c.rounded(Rect::new(x0, cy - thick, at - x0, thick * 2.0), thick, lit);
        c.circle(at, cy, thumb, lit);
    }
}

/// A switch's size, of its button's height.
const SWITCH_H: f32 = 0.42;
const SWITCH_W: f32 = 0.85;
/// The knob's radius, of the switch's height.
const KNOB: f32 = 0.38;

/// An on/off switch, centred in its button `r`: green with the knob right when on.
pub(super) fn switch(f: &mut Frame, id: Id, r: Rect, on: bool) {
    f.button(id, r, Look::ICON, |f| switch_shape(f.c, r, on));
}

/// A switch's drawing, centred in `r` (sized by its height).
pub(super) fn switch_shape(c: &mut Canvas, r: Rect, on: bool) {
    let (h, w) = (r.h * SWITCH_H, r.h * SWITCH_W);
    let track = r.centered(w, h);
    c.rounded(track, h / 2.0, if on { Tone::Go.rgb() } else { SURFACE_HI });
    let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
    c.circle(knob_x, track.y + h / 2.0, h * KNOB, if on { Tone::High } else { Tone::Dim });
}

/// What a status row shows: its icon and name (in `tone`), a lock before the state if `lock`, and the
/// state in its own tone.
pub(super) struct RowFace<'a> {
    pub icon: Icon,
    pub name: &'a str,
    pub tone: Tone,
    pub lock: bool,
    pub state: &'a str,
    pub state_tone: Tone,
}

/// A status row's icon, of its height; the state's column, of its width.
const ROW_ICON: f32 = 0.42;
const STATE_W: f32 = 0.22;

/// A row of a list of things with a state (networks, devices): the icon, the name (cut to fit), and
/// the state on the right. `pad` is the list's padding, `size` its text size.
pub(super) fn status_row(f: &mut Frame, id: Id, r: Rect, pad: f32, size: f32, face: RowFace) {
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
        // Smaller, its top on the name's.
        let ty = r.y + (r.h - size * LINE) / 2.0;
        f.c.text(state_x + icon, ty, size * 0.85, face.state_tone, face.state);
    });
}

/// Where a tile's picture goes, of the tile's height: its size and its top; and whether a caption too
/// long for the tile is cut (leaving this much room at its sides together, in logical pixels) or runs
/// over its edges.
pub(super) struct TileShape {
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

/// What a tile shows besides its picture: the caption under it, a dot in the top right corner (the
/// game's own setting, the game in front), small text in the top left (dim) and top right corners.
pub(super) struct TileFace<'a> {
    pub caption: &'a str,
    pub caption_tone: Tone,
    pub dot: Option<Tone>,
    pub note: &'a str,
    pub count: &'a str,
}

/// The caption's size and top, of the tile's height; the corner texts' sizes.
const CAPTION: f32 = 0.15;
pub(super) const CAPTION_Y: f32 = 0.62;
const NOTE: f32 = 0.09;
const COUNT: f32 = 0.11;

/// A tile of a grid (settings, the dock): no background, `picture` (given the tile) over the caption.
pub(super) fn tile(f: &mut Frame, id: Id, r: Rect, shape: &TileShape, face: TileFace, picture: impl FnOnce(&mut Frame, Rect)) {
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
        if !face.count.is_empty() {
            let size = r.h * COUNT;
            let w = f.c.text_width(size, face.count);
            f.c.text(r.x + r.w - w - r.h * 0.06, r.y + r.h * 0.05, size, Tone::Normal, face.count);
        }
    });
}

/// The game's box art fitted into `r` (centred across it; down it as `align` says, `Start` at the top),
/// or a placeholder in its colour while there is none.
pub(super) fn art(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, radius: f32, align: Align) {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    match game.box_front.as_deref().and_then(|path| images.get(&game.id, path, pw, ph)) {
        Some(img) => {
            let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
            let y = match align {
                Align::Start => r.y,
                Align::Center => r.y + (r.h - ih) / 2.0,
                Align::End => r.y + r.h - ih,
            };
            c.image(img, r.x + (r.w - iw) / 2.0, y);
        }
        None => c.rounded(r, radius, Rgb(game.color)),
    }
}

/// A screen's header band, of the body's height; a menu's and a list's text buttons, and a menu's width,
/// of the body's height and the area's width.
const HEADER_H: f32 = 0.1;
const MENU_BUTTON_H: f32 = 0.085;
const MENU_W: f32 = 0.45;

/// What a header has on its right: nothing, or an action (blue text).
pub(super) enum HeaderRight<'a> {
    None,
    Action(Id, &'a str),
}

/// A screen's header over `body`: a back arrow (`back`, its target) on the left, the title (from the
/// left after it, or centred), and on the right an action. Returns the area below it.
pub(super) fn header(f: &mut Frame, body: Rect, back: Option<Id>, title: &str, centred: bool, right: HeaderRight) -> Rect {
    let pad = style::pad(body.h);
    let mut area = style::content(body);
    let mut bar = area.take_top(body.h * HEADER_H);
    let full = bar;
    let size = Text::Heading.size(body.h);
    let left_w = match back {
        Some(id) => {
            let r = bar.take_left(bar.h);
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

/// A menu: text actions down `area` from its top (each `(target, label, tone)`), as many as fit.
pub(super) fn menu(f: &mut Frame, area: Rect, body: Rect, items: &[(Id, &str, Tone)]) {
    let pad = style::pad(body.h);
    let h = body.h * MENU_BUTTON_H;
    let size = Text::Body.size(body.h);
    for (i, (id, label, tone)) in items.iter().enumerate() {
        let r = Rect::new(area.x, area.y + i as f32 * (h + pad * 0.5), menu_w(area, size), h);
        if r.bottom() > area.bottom() {
            break;
        }
        f.text_button(id.clone(), r, size, *tone, label, Align::Start);
    }
}

/// A menu's width in `area`, for text at `size`.
pub(super) fn menu_w(area: Rect, size: f32) -> f32 {
    (area.w * MENU_W).max(size * 6.0).min(area.w)
}

/// A list's row: its label (in `tone`, after a dot if it is the one in force), a note in a column on
/// the right (dim), and at the end an Edit action (its target, a target of its own), or its room kept
/// empty (so the notes line up in a list where other rows have one).
pub(super) struct ListRow<'a> {
    pub label: &'a str,
    pub tone: Tone,
    pub mark: bool,
    pub note: &'a str,
    pub edit: EditCell,
}

/// A list row's end: nothing, an empty Edit column, or Edit.
pub(super) enum EditCell {
    None,
    Empty,
    Edit(Id),
}

/// A list row's note column and Edit action, of the row's width.
const NOTE_W: f32 = 0.36;
const EDIT_W: f32 = 0.14;

/// A list's row (see [`ListRow`]) over `r`, its main target `id`, its text at `size`.
pub(super) fn list_row(f: &mut Frame, id: Id, mut r: Rect, pad: f32, size: f32, row: ListRow) {
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
        let note_w = if row.note.is_empty() { 0.0 } else { r.w * NOTE_W };
        f.c.text_fit(Rect::new(x, r.y, r.x + r.w - note_w - x - pad * 0.5, r.h), size, row.tone, row.label, Keep::Start);
        if !row.note.is_empty() {
            f.c.text_fit(Rect::new(r.x + r.w - note_w, r.y, note_w - pad * 0.5, r.h), size * 0.8, Tone::Dim, row.note, Keep::Start);
        }
    });
    if let Some((id, e)) = edit {
        let tone = if f.pressed(&id) { Tone::High } else { Tone::Fn };
        f.button(id, e, Look::ROW, |f| {
            f.c.text_in(e, size * 0.85, tone, "Edit", Align::Center);
        });
    }
}

/// A switch's width in a row, for the row's text size.
pub(super) fn switch_w(size: f32) -> f32 {
    size * 2.3
}

/// A switch's row: `label` on the left (dim), the switch (`on`) on the right; the whole row its target.
pub(super) fn switch_row(f: &mut Frame, id: Id, row: Rect, size: f32, label: &str, on: bool) {
    f.button(id, row, Look::ROW, |f| {
        let inner = row.inset(size * 0.4);
        let sw = Rect::new(inner.right() - switch_w(size), row.y, switch_w(size), row.h);
        f.c.text_fit(Rect::new(inner.x, row.y, sw.x - inner.x, row.h), size, Tone::Dim, label, Keep::Start);
        switch_shape(f.c, sw, on);
    });
}

/// A value's row: `label` on the left (dim), `value` on the right (brighter when `changed` from its
/// default). Its target is the caller's (a held slider's, a choice's).
pub(super) fn value_row(f: &mut Frame, row: Rect, size: f32, label: &str, value: &str, changed: bool) {
    let inner = row.inset(size * 0.4);
    let vw = f.c.text_width(size, value).min(inner.w * 0.5);
    f.c.text_fit(Rect::new(inner.x, row.y, inner.w - vw - size * 0.3, row.h), size, Tone::Dim, label, Keep::Start);
    f.c.text_fit(Rect::new(inner.right() - vw, row.y, vw + 1.0, row.h), size, Tone::high_if(changed), value, Keep::Start);
}

/// Text centred in `r` (cut to fit).
pub(super) fn centred_text(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
    let w = f.c.text_width(size, text).min(r.w);
    f.c.text_fit(Rect::new(r.x + (r.w - w) / 2.0, r.y, w + 1.0, r.h), size, tone, text, Keep::Start);
}

/// Text centred in `r`, on two lines (split at a space near the middle) if it doesn't fit on one.
pub(super) fn centred_lines(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
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
