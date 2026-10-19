//! Pieces more than one view draws: a screen's header, a menu of text actions, a list's row, a row of
//! tabs, a level's track, a held slider and its card, a switch and a switch's row, a value's row, a
//! status row (a network, a device), a tile (an icon over a caption), a game's box art, and centred
//! text.

use clamshell_core::{Game, Id};

use super::scroll::Scrolled;
use super::style::{self, Look, SURFACE_HI, Text, Tone};
use super::{Frame, Track};
use crate::draw::{Align, Canvas, Column, Keep, LINE, Rect, Rgb};
use crate::icons::Icon;
use crate::images::{Images, Slot};

/// A row of tabs from `area.x`, `area.h` high: each its icon (if it has one) and label (cut to `max`
/// wide) with `inner` space on both sides; the current one bright with an amber line under it. Tabs that
/// would pass the area's right edge are left out. Returns where a next one would start.
pub(crate) fn tabs<'a>(f: &mut Frame, area: Rect, size: f32, inner: f32, max: f32, items: impl IntoIterator<Item = (Id, Option<Icon>, &'a str, bool)>) -> f32 {
    let mut x = area.x;
    // An icon: about the label's capitals high, a space after it.
    let (icon_w, gap) = (size * 1.1, size * 0.35);
    for (id, icon, name, current) in items {
        let lead = if icon.is_some() { icon_w + gap } else { 0.0 };
        let text_w = f.c.text_width(size, name).min(max);
        let r = Rect::new(x, area.y, lead + text_w + 2.0 * inner, area.h);
        if r.x + r.w > area.x + area.w {
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
pub(crate) const SHADE: Rgb = SURFACE_HI;
pub(crate) const SHADE_ALPHA: f32 = 0.85;

/// A held slider (volume and brightness over the bar, a keymap parameter in its row): the slider, the
/// x its track's positions count from, the height of its middle, its size (the bar's height, a row's),
/// the room its card may use, and what the card says.
pub(crate) struct HeldSlider<'a> {
    pub slider: &'a clamshell_core::slider::Slider,
    pub x0: f32,
    pub cy: f32,
    pub h: f32,
    pub within: Rect,
    pub icon: Option<Icon>,
    pub text: &'a str,
}

/// A held slider (see [`HeldSlider`]): on a shaded band at its height, the track laid out with the value
/// it had under the finger (the value lit, the thumb where it is now), and the value on a small card of
/// its own where the finger doesn't cover it: below the slider (above it without room below), to the
/// side away from the finger.
pub(crate) fn held_slider(f: &mut Frame, held: HeldSlider) {
    let HeldSlider { slider, x0, cy, h, within, icon, text } = held;
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
pub(crate) fn card_width(f: &mut Frame, h: f32, icon: bool, text: &str) -> f32 {
    let icon_w = if icon { h * SLIDER_ICON * 1.3 } else { 0.0 };
    icon_w + f.c.text_width(h * SLIDER_TEXT, text)
}

/// A small card of its own over whatever is under it (shaded): an icon and a line of text, its content
/// from `x` at `y`, sized by `h` (the bar's height, a panel row's). A held slider's value and the bar's
/// notices.
pub(crate) fn card(f: &mut Frame, x: f32, y: f32, h: f32, icon: Option<Icon>, text: &str) {
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
pub(crate) fn track(c: &mut Canvas, track: Track, cy: f32, thick: f32, thumb: f32, value: Option<u8>, lit: Tone) {
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
pub(crate) fn switch(f: &mut Frame, id: Id, r: Rect, on: bool) {
    f.button(id, r, Look::ICON, |f| switch_shape(f.c, r, on));
}

/// A switch's drawing, centred in `r` (sized by its height).
pub(crate) fn switch_shape(c: &mut Canvas, r: Rect, on: bool) {
    let (h, w) = (r.h * SWITCH_H, r.h * SWITCH_W);
    let track = r.centered(w, h);
    c.rounded(track, h / 2.0, if on { Tone::Go.rgb() } else { SURFACE_HI });
    let knob_x = if on { track.x + track.w - h / 2.0 } else { track.x + h / 2.0 };
    c.circle(knob_x, track.y + h / 2.0, h * KNOB, if on { Tone::High } else { Tone::Dim });
}

/// What a status row shows: its icon and name (in `tone`), a lock before the state if `lock`, and the
/// state in its own tone.
pub(crate) struct RowFace<'a> {
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
pub(crate) fn status_row(f: &mut Frame, id: Id, r: Rect, pad: f32, size: f32, face: RowFace) {
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
pub(crate) struct TileShape {
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
pub(crate) struct TileFace<'a> {
    pub caption: &'a str,
    pub caption_tone: Tone,
    pub dot: Option<Tone>,
    pub note: &'a str,
}

/// The caption's size and top, of the tile's height; the corner note's size.
const CAPTION: f32 = 0.15;
pub(crate) const CAPTION_Y: f32 = 0.62;
const NOTE: f32 = 0.09;

/// A tile of a grid (settings, the dock): no background, `picture` (given the tile) over the caption.
pub(crate) fn tile(f: &mut Frame, id: Id, r: Rect, shape: &TileShape, face: TileFace, picture: impl FnOnce(&mut Frame, Rect)) {
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

/// An image drawn grey (an entry not running, on the dock): at this brightness.
const GREY: f32 = 0.55;

/// A colour as an image is drawn grey: its luminance at [`GREY`]'s brightness.
fn greyed(color: u32) -> u32 {
    let channel = |shift: u32| ((color >> shift) & 0xff) as f32;
    let luma = (0.3 * channel(16) + 0.59 * channel(8) + 0.11 * channel(0)) * GREY;
    let v = luma.round().clamp(0.0, 255.0) as u32;
    (v << 16) | (v << 8) | v
}

/// An entry's picture: its box art; without art or an icon, its screenshot (a video's frame).
pub(crate) fn picture(game: &Game) -> Option<(&std::path::Path, Slot)> {
    match (&game.box_front, &game.icon, &game.shot) {
        (Some(art), _, _) => Some((art, Slot::Art)),
        (None, None, Some(shot)) => Some((shot, Slot::Shot)),
        _ => None,
    }
}

/// Whether an entry's picture is its screenshot (a video's frame: landscape).
pub(crate) fn wide_picture(game: &Game) -> bool {
    matches!(picture(game), Some((_, Slot::Shot)))
}

/// An entry's box art as large as fits in `r` (portrait or landscape), at its top left; without art or an
/// icon, its screenshot (a video's frame) instead; where it has none of them (or it hasn't loaded), the
/// placeholder (or an app's icon) in a portrait box as high as `r`. The rectangle drawn.
pub(crate) fn cover(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, radius: f32, portrait: f32) -> Rect {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    if let Some(img) = picture(game).and_then(|(path, slot)| images.get(&game.id, slot, path, pw, ph)) {
        let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
        c.image(img, r.x, r.y);
        return Rect::new(r.x, r.y, iw, ih);
    }
    let box_rect = Rect::new(r.x, r.y, (r.h * portrait).min(r.w), r.h);
    art(c, images, game, box_rect, radius, Align::Start, false);
    box_rect
}

/// How far into a video: a track `r` wide, the part watched (`fraction`) in amber.
pub(crate) fn progress(c: &mut Canvas, r: Rect, fraction: f32) {
    c.rounded(r, r.h / 2.0, SURFACE_HI);
    let done = (r.w * fraction.clamp(0.0, 1.0)).max(r.h);
    c.rounded(Rect::new(r.x, r.y, done, r.h), r.h / 2.0, Tone::Accent);
}

/// An entry's screenshot fitted into `r`, at its bottom left, if it has one and it has loaded; nothing
/// meanwhile.
pub(crate) fn shot(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect) {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    if let Some(img) = game.shot.as_deref().and_then(|path| images.get(&game.id, Slot::Shot, path, pw, ph)) {
        c.image(img, r.x, r.y + r.h - img.height() as f32 / scale);
    }
}

/// An entry's picture in `r`: its box art fitted in (centred across it; down it as `align` says, `Start`
/// at the top), else its icon (see [`icon`]); `grey` for one shown stopped.
pub(crate) fn art(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, radius: f32, align: Align, grey: bool) {
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    match picture(game).and_then(|(path, slot)| images.get(&game.id, slot, path, pw, ph)) {
        Some(img) => {
            let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
            let y = match align {
                Align::Start => r.y,
                Align::Center => r.y + (r.h - ih) / 2.0,
                Align::End => r.y + r.h - ih,
            };
            if grey {
                c.image_grey(img, r.x + (r.w - iw) / 2.0, y, GREY);
            } else {
                c.image(img, r.x + (r.w - iw) / 2.0, y);
            }
        }
        None => icon(c, images, game, r, radius, grey),
    }
}

/// A list row's picture in `r` (a square): a folder's icon where it has one (its picture is for the
/// details), else its picture (box art, a video's frame) cropped to fill `r`, its middle kept; else its
/// icon.
pub(crate) fn thumb(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, radius: f32, grey: bool) {
    if game.is_group() && (game.icon.is_some() || game.glyph.is_some()) {
        return icon(c, images, game, r, radius, grey);
    }
    let scale = c.scale();
    let (pw, ph) = ((r.w * scale).round() as u32, (r.h * scale).round() as u32);
    match picture(game).and_then(|(path, _)| images.get(&game.id, Slot::Thumb, path, pw, ph)) {
        Some(img) if grey => c.image_grey(img, r.x, r.y, GREY),
        Some(img) => c.image(img, r.x, r.y),
        None => icon(c, images, game, r, radius, grey),
    }
}

/// An entry's icon in `r`, square in the middle: its image (an app's, a folder's), else one of the UI's by
/// name (a browsed entry's: a folder, a file), else for an app the UI's app glyph, else (a game) a
/// placeholder in its colour.
pub(crate) fn icon(c: &mut Canvas, images: &mut Images, game: &Game, r: Rect, radius: f32, grey: bool) {
    let scale = c.scale();
    let side = (r.w.min(r.h) * 0.8 * scale).round() as u32;
    if let Some(img) = game.icon.as_deref().and_then(|path| images.get(&game.id, Slot::Icon, path, side, side)) {
        let (iw, ih) = (img.width() as f32 / scale, img.height() as f32 / scale);
        let (x, y) = (r.x + (r.w - iw) / 2.0, r.y + (r.h - ih) / 2.0);
        if grey {
            c.image_grey(img, x, y, GREY);
        } else {
            c.image(img, x, y);
        }
        return;
    }
    match game.glyph.as_deref().and_then(Icon::named).or(game.tool.then_some(Icon::App)) {
        Some(glyph) => {
            let side = r.w.min(r.h) * 0.7;
            c.icon(glyph, Rect::new(r.x + (r.w - side) / 2.0, r.y + (r.h - side) / 2.0, side, side), if grey { Tone::Dim } else { Tone::Normal });
        }
        None => c.rounded(r, radius, Rgb(if grey { greyed(game.color) } else { game.color })),
    }
}

/// A screen's header band, of the body's height; a menu's and a list's text buttons, and a menu's width,
/// of the body's height and the area's width.
const HEADER_H: f32 = 0.1;
const MENU_BUTTON_H: f32 = 0.085;
const MENU_W: f32 = 0.45;

/// What a header has on its right: nothing, or an action (blue text).
pub(crate) enum HeaderRight<'a> {
    None,
    Action(Id, &'a str),
}

/// A screen's header, at the top of the content area: a back arrow (`back`, its target) on the left, the title (from the
/// left after it, or centred), and on the right an action. Returns the area below it.
pub(crate) fn header(f: &mut Frame, back: Option<Id>, title: &str, centred: bool, right: HeaderRight) -> Rect {
    let pad = f.pad();
    let mut area = style::content(f.body());
    let mut bar = area.take_top(f.body().h * HEADER_H);
    let full = bar;
    let size = f.text(Text::Heading);
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

/// A menu: text actions down `area` from its top (each `(target, label, tone)`), in a column beside the
/// first when they don't fit (a long one: a program's tray menu), as many as fit.
pub(crate) fn menu(f: &mut Frame, area: Rect, items: &[(Id, &str, Tone)]) {
    let pad = f.pad();
    let h = f.body().h * MENU_BUTTON_H;
    let size = f.text(Text::Body);
    let w = menu_w(area, size);
    let per_column = (((area.h + pad * 0.5) / (h + pad * 0.5)) as usize).max(1);
    for (i, (id, label, tone)) in items.iter().enumerate() {
        let (column, row) = (i / per_column, i % per_column);
        let r = Rect::new(area.x + column as f32 * (w + pad), area.y + row as f32 * (h + pad * 0.5), w, h);
        if r.right() > area.right() {
            break;
        }
        f.text_button(id.clone(), r, size, *tone, label, Align::Start);
    }
}

/// A menu's width in `area`, for text at `size`.
pub(crate) fn menu_w(area: Rect, size: f32) -> f32 {
    (area.w * MENU_W).max(size * 6.0).min(area.w)
}

/// A list's row: its label (in `tone`, after a dot if it is the one in force), a note in a column on
/// the right (dim), and at the end an Edit action (its target, a target of its own), or its room kept
/// empty (so the notes line up in a list where other rows have one).
pub(crate) struct ListRow<'a> {
    pub label: &'a str,
    pub tone: Tone,
    pub mark: bool,
    pub note: &'a str,
    pub edit: EditCell,
}

/// A list row's end: nothing, an empty Edit column, or Edit.
pub(crate) enum EditCell {
    None,
    Empty,
    Edit(Id),
}

/// A list row's note column and Edit action, of the row's width.
const NOTE_W: f32 = 0.36;
const EDIT_W: f32 = 0.14;

/// A list's row (see [`ListRow`]) over `r`, its main target `id`, its text at `size`.
pub(crate) fn list_row(f: &mut Frame, id: Id, mut r: Rect, pad: f32, size: f32, row: ListRow) {
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
pub(crate) fn switch_w(size: f32) -> f32 {
    size * 2.3
}

/// A switch's row: `label` on the left (dim), the switch (`on`) on the right; the whole row its target.
pub(crate) fn switch_row(f: &mut Frame, id: Id, row: Rect, size: f32, label: &str, on: bool) {
    f.button(id, row, Look::ROW, |f| {
        let inner = row.inset(size * 0.4);
        let sw = Rect::new(inner.right() - switch_w(size), row.y, switch_w(size), row.h);
        f.c.text_fit(Rect::new(inner.x, row.y, sw.x - inner.x, row.h), size, Tone::Dim, label, Keep::Start);
        switch_shape(f.c, sw, on);
    });
}

/// A value's row: `label` on the left (dim), `value` on the right (brighter when `changed` from its
/// default). Its target is the caller's (a held slider's, a choice's).
pub(crate) fn value_row(f: &mut Frame, row: Rect, size: f32, label: &str, value: &str, changed: bool) {
    let inner = row.inset(size * 0.4);
    let vw = f.c.text_width(size, value).min(inner.w * 0.5);
    f.c.text_fit(Rect::new(inner.x, row.y, inner.w - vw - size * 0.3, row.h), size, Tone::Dim, label, Keep::Start);
    f.c.text_fit(Rect::new(inner.right() - vw, row.y, vw + 1.0, row.h), size, Tone::high_if(changed), value, Keep::Start);
}

/// An action and the pad button that does it, a target filling `r`: the button's name in a ring (a pill
/// for a longer one), then the label (in `tone`).
pub(crate) fn key_action(f: &mut Frame, id: Id, r: Rect, key: &str, label: &str, tone: Tone, faded: bool) {
    f.button(id, r, Look::ICON, |f| {
        key_label(f, r, key, label, tone, faded);
    });
}

/// A button's badge and what it does, as [`key_action`] draws them, but nothing to tap: a hint. Returns
/// its width.
pub(crate) fn key_hint(f: &mut Frame, r: Rect, key: &str, label: &str) -> f32 {
    key_label(f, r, key, label, Tone::Dim, false)
}

/// How strongly a faded badge (its ring and letter) shows.
const FADED: f32 = 0.35;

/// The badge (a ring with the button's name; none without one: the label where the others' are; faded
/// while the button doesn't do it) and the label after it; their width.
fn key_label(f: &mut Frame, r: Rect, key: &str, label: &str, tone: Tone, faded: bool) -> f32 {
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

/// Text centred in `r` (cut to fit).
pub(crate) fn centred_text(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
    let w = f.c.text_width(size, text).min(r.w);
    f.c.text_fit(Rect::new(r.x + (r.w - w) / 2.0, r.y, w + 1.0, r.h), size, tone, text, Keep::Start);
}

/// Text centred in `r`, on two lines (split at a space near the middle) if it doesn't fit on one.
pub(crate) fn centred_lines(f: &mut Frame, r: Rect, size: f32, tone: Tone, text: &str) {
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

/// A hint's text, of the body text.
const HINT: f32 = 0.8;

/// A hint line's height (small dim text at the bottom of a screen).
pub(crate) fn hint_h(f: &Frame) -> f32 {
    f.text(Text::Body) * HINT * LINE * 1.25
}

/// A hint along the bottom of `area` (small, dim); returns the room above it.
pub(crate) fn hint(f: &mut Frame, mut area: Rect, text: &str) -> Rect {
    let h = hint_h(f);
    let line = area.take_bottom(h);
    let size = f.text(Text::Body) * HINT;
    f.c.text_fit(line.top_line(size), size, Tone::Dim, text, Keep::Start);
    area
}

/// A screen's scrolled list (networks, devices, keymaps, layers): its rows in `area` above a hint line,
/// each reaching a little into the side margins (for its pressed look), the scroll bar just right of
/// them.
pub(crate) struct ListArea {
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

    /// The rows' list begun (see [`Scrolled`]): `len` rows above the hint line.
    pub fn scrolled(&self, f: &mut Frame, list: clamshell_core::ListId, len: usize) -> Scrolled {
        let view = Rect::new(self.area.x, self.area.y, self.area.w, self.area.h - hint_h(f) - self.pad * 0.3).widened(self.pad * 0.5);
        Scrolled::begin(f, list, view, self.row_h, len)
    }

    /// The list drawn: its scroll bar just right of the rows.
    pub fn end(&self, f: &mut Frame, rows: Scrolled) {
        rows.end(f, self.area.right() + self.pad * 0.6);
    }

    /// The hint at the bottom.
    pub fn hint(&self, f: &mut Frame, text: &str) {
        hint(f, self.area, text);
    }

    /// A dim line where the rows would be (nothing found yet).
    pub fn empty(&self, f: &mut Frame, text: &str) {
        f.c.text(self.area.x, self.area.y + self.pad, self.size, Tone::Dim, text);
    }
}

/// Lines of text centred across `col`'s width, one under another, each `(size, tone, text)`, as many as
/// fit.
pub(crate) fn text_lines(f: &mut Frame, col: &mut Column, lines: &[(f32, Tone, &str)]) {
    for &(size, tone, text) in lines {
        let Some(r) = col.next(size * LINE) else {
            return;
        };
        centred_text(f, r, size, tone, text);
    }
}
