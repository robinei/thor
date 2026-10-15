//! The volume and brightness tabs. Volume: the output volume with its mute button, which output plays
//! (a tap chooses one), then each app playing with its own level and mute button. Brightness: the level
//! (the bar's), then each screen's backlight with its share of it (the two multiply). A level row is set
//! by where the finger is on its track (and follows it); the pad's left and right step it.

use std::fmt::Write;

use clamshell_core::levels::Level;
use clamshell_core::{AudioId, Id, State};

use super::bar::volume_icon;
use super::{ACCENT, DIM, Frame, GO, Look, TEXT, TEXT_HI};
use crate::draw::{Keep, Rect, rgb};
use crate::icons::Icon;

/// Padding, a row's height, text sizes, of the area's height.
const PAD: f32 = 0.03;
const ROW_H: f32 = 0.11;
const TEXT_SIZE: f32 = 0.04;
const HEADING: f32 = 0.032;
/// A level row: the name's column and the number's, of the area's width; the track's thickness and
/// thumb, of the row's height; icons, of the row's height.
const NAME_W: f32 = 0.28;
/// The brightness tab's name column (backlight names are long).
const BACKLIGHT_NAME_W: f32 = 0.4;
const NUMBER_W: f32 = 0.09;
/// A screen's share ("100 %"), of the area's width.
const PERCENT_W: f32 = 0.13;
const TRACK: f32 = 0.06;
const THUMB: f32 = 0.16;
const ICON: f32 = 0.5;

const ROW: Look = Look { radius: 8.0, fill: None, inset: 1.0, ring_inset: 0.0 };

/// A level's track over `r` (the whole width is 0 to 100): lit up to the level, the thumb at it; dim
/// without a level or while muted.
fn track(f: &mut Frame, level: Level, r: Rect, muted: bool) {
    let value = f.s.levels().level(level);
    f.button(Id::Level(level), r, ROW, |c| {
        let (h, cy) = (r.h * TRACK, r.y + r.h / 2.0);
        let thumb = r.h * THUMB;
        // Inset by the thumb, so it stays within the row at 0 and 100.
        let (x0, w) = (r.x + thumb, r.w - 2.0 * thumb);
        c.rounded(Rect::new(x0, cy - h / 2.0, w, h), h / 2.0, rgb(DIM));
        if let Some(v) = value {
            let at = x0 + w * f32::from(v) / 100.0;
            let lit = if muted { DIM } else { TEXT_HI };
            c.rounded(Rect::new(x0, cy - h, at - x0, h * 2.0), h, rgb(lit));
            c.circle(at, cy, thumb, rgb(lit));
        }
    });
}

/// The level the finger at `x` sets on a track drawn over `r`.
pub(crate) fn value_at(r: Rect, x: f32) -> u8 {
    let thumb = r.h * THUMB;
    let v = (x - r.x - thumb) / (r.w - 2.0 * thumb).max(1.0) * 100.0;
    v.round().clamp(0.0, 100.0) as u8
}

/// The level as a number in `r`, right-aligned (`suffix` after it); "-" when unknown.
fn number(f: &mut Frame, r: Rect, value: Option<u8>, suffix: &str, size: f32, buf: &mut String) {
    buf.clear();
    match value {
        Some(v) => {
            let _ = write!(buf, "{v}{suffix}");
        }
        None => buf.push('-'),
    }
    let w = f.c.text_width(size, buf);
    f.c.text(r.x + r.w - w, r.y + (r.h - size * 1.2) / 2.0, size, rgb(TEXT), buf);
}

/// A small heading over a group of rows.
fn heading(f: &mut Frame, x: f32, y: f32, size: f32, text: &str) -> f32 {
    f.c.text(x, y, size, rgb(DIM), text);
    size * 1.8
}

pub(super) fn draw_volume(f: &mut Frame, body: Rect, buf: &mut String) {
    let s: &State = f.s;
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y + pad * 0.5, body.w - 2.0 * pad, body.h - pad);
    let (row_h, size) = (body.h * ROW_H, body.h * TEXT_SIZE);
    let side = row_h * ICON;
    let name_w = area.w * NAME_W;
    let number_w = area.w * NUMBER_W;

    // The output volume: mute, track, number.
    let row = Rect::new(area.x, area.y, area.w, row_h);
    let mute = Rect::new(row.x, row.y, name_w, row.h);
    let muted = s.levels().muted();
    f.button(Id::Audio(AudioId::MuteOutput), mute, ROW, |c| {
        let icon = volume_icon(s.levels().volume().unwrap_or(0), muted);
        let color = if muted { ACCENT } else { TEXT };
        c.icon(icon, Rect::new(mute.x + pad * 0.5, mute.y + (mute.h - side) / 2.0, side, side), rgb(color));
        let ty = mute.y + (mute.h - size * 1.2) / 2.0;
        c.text(mute.x + pad * 0.5 + side * 1.4, ty, size, rgb(color), if muted { "Muted" } else { "Volume" });
    });
    track(f, Level::Volume, Rect::new(row.x + name_w, row.y, row.w - name_w - number_w, row.h), muted);
    number(f, Rect::new(row.x + row.w - number_w, row.y, number_w, row.h), s.levels().volume(), "", size, buf);
    let mut y = row.y + row.h + pad * 0.5;
    let bottom = area.y + area.h;

    // Where the sound goes.
    let hsize = body.h * HEADING;
    y += heading(f, area.x, y, hsize, "Output");
    if s.levels().audio().outputs.is_empty() {
        f.c.text(area.x, y, size, rgb(DIM), "No outputs");
        y += row_h * 0.6;
    }
    // (Room kept below for the apps' heading and a row.)
    let apps_room = hsize * 1.8 + row_h + pad;
    for (i, o) in s.levels().audio().outputs.iter().enumerate() {
        if y + row_h * 0.8 > bottom - apps_room {
            break;
        }
        let r = Rect::new(area.x - pad * 0.5, y, area.w + pad, row_h * 0.8);
        f.button(Id::Audio(AudioId::Output(i as u16)), r, ROW, |c| {
            let dot = r.h * 0.09;
            let color = if o.default { TEXT_HI } else { TEXT };
            if o.default {
                c.circle(r.x + pad * 0.5 + dot * 2.0, r.y + r.h / 2.0, dot, rgb(GO));
            }
            let x = r.x + pad * 0.5 + dot * 5.0;
            c.text_fit((x, r.y + (r.h - size * 1.2) / 2.0, r.x + r.w - x - pad), size, rgb(color), &o.name, Keep::Start);
        });
        y += r.h;
    }

    // Each app playing.
    y += pad * 0.5;
    if y + hsize * 1.8 > bottom {
        return;
    }
    y += heading(f, area.x, y, hsize, "Apps");
    if s.levels().audio().streams.is_empty() {
        f.c.text(area.x, y, size, rgb(DIM), "Nothing is playing");
    }
    for (i, stream) in s.levels().audio().streams.iter().enumerate() {
        if y + row_h > bottom {
            break;
        }
        let row = Rect::new(area.x, y, area.w, row_h);
        let ty = row.y + (row.h - size * 1.2) / 2.0;
        let color = if stream.muted { DIM } else { TEXT };
        f.c.text_fit((row.x, ty, name_w - pad), size, rgb(color), &stream.app, Keep::Start);
        let i = i as u16;
        track(f, Level::Stream(i), Rect::new(row.x + name_w, row.y, row.w - name_w - number_w, row.h), stream.muted);
        let m = Rect::new(row.x + row.w - number_w, row.y, number_w, row.h);
        f.button(Id::Audio(AudioId::StreamMute(i)), m, ROW, |c| {
            let icon = volume_icon(stream.volume, stream.muted);
            let color = if stream.muted { ACCENT } else { TEXT };
            c.icon(icon, Rect::new(m.x + (m.w - side) / 2.0, m.y + (m.h - side) / 2.0, side, side), rgb(color));
        });
        y += row_h;
    }
}

pub(super) fn draw_brightness(f: &mut Frame, body: Rect, buf: &mut String) {
    let s: &State = f.s;
    let pad = body.h * PAD;
    let area = Rect::new(body.x + pad, body.y + pad * 0.5, body.w - 2.0 * pad, body.h - pad);
    let (row_h, size) = (body.h * ROW_H, body.h * TEXT_SIZE);
    let side = row_h * ICON;
    let name_w = area.w * BACKLIGHT_NAME_W;
    let number_w = area.w * NUMBER_W;

    // The level (the bar's): every screen follows it.
    let row = Rect::new(area.x, area.y, area.w, row_h);
    let ty = row.y + (row.h - size * 1.2) / 2.0;
    f.c.icon(Icon::Brightness, Rect::new(row.x, row.y + (row.h - side) / 2.0, side, side), rgb(TEXT));
    f.c.text(row.x + side * 1.4, ty, size, rgb(TEXT), "Brightness");
    track(f, Level::Brightness, Rect::new(row.x + name_w, row.y, row.w - name_w - number_w, row.h), false);
    number(f, Rect::new(row.x + row.w - number_w, row.y, number_w, row.h), s.levels().level(Level::Brightness), "", size, buf);
    let mut y = row.y + row.h + pad * 0.5;

    // Each screen's share of it.
    y += heading(f, area.x, y, body.h * HEADING, "Each screen, of the brightness above");
    if s.levels().backlights().is_empty() {
        f.c.text(area.x, y, size, rgb(DIM), "No backlight");
        return;
    }
    for (i, name) in s.levels().backlights().iter().enumerate() {
        if y + row_h > area.y + area.h {
            break;
        }
        let row = Rect::new(area.x, y, area.w, row_h);
        let ty = row.y + (row.h - size * 1.2) / 2.0;
        f.c.text_fit((row.x, ty, name_w - pad), size, rgb(TEXT), name, Keep::Start);
        let level = Level::Backlight(i as u16);
        let percent_w = area.w * PERCENT_W;
        track(f, level, Rect::new(row.x + name_w, row.y, row.w - name_w - percent_w, row.h), false);
        number(f, Rect::new(row.x + row.w - percent_w, row.y, percent_w, row.h), s.levels().level(level), " %", size, buf);
        y += row_h;
    }
}
